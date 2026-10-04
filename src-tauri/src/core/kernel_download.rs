//! sing-box 内核分发（P0：首启自动下载）
//!
//! 生产形态后续可改为 Tauri sidecar 随安装包分发；当前策略：
//! 首次启动内核时，若 work_dir 下没有 sing-box 二进制，则自动从官方
//! GitHub Release 下载（带镜像回退），解压后落盘为 work_dir/sing-box[-exe]。
//!
//! 全过程通过 "kernel-download://progress" 事件向前端推送进度：
//!   { "stage": "download" | "extract" | "ready" | "error",
//!     "percent": 0-100, "message": "..." }

use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt as _;

/// 跟随官方稳定版升级；手动放置的同名二进制优先，不会被覆盖
pub const KERNEL_VERSION: &str = "1.11.3";

/// 下载进度事件名
pub const PROGRESS_EVENT: &str = "kernel-download://progress";

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Progress<'a> {
    stage: &'a str,
    percent: u8,
    message: String,
}

fn emit(app: &AppHandle, stage: &str, percent: u8, message: impl Into<String>) {
    let payload = Progress {
        stage,
        percent,
        message: message.into(),
    };
    if app.emit(PROGRESS_EVENT, payload).is_err() {
        tracing::debug!("kernel download progress event has no listeners");
    }
}

/// work_dir 下的内核二进制最终路径
pub fn kernel_binary_path(work_dir: &Path) -> PathBuf {
    let exe = if cfg!(windows) {
        "sing-box.exe"
    } else {
        "sing-box"
    };
    work_dir.join(exe)
}

/// 目标平台对应的发行包文件名
fn asset_name() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Ok("sing-box-1.11.3-windows-amd64.zip"),
        ("windows", "aarch64") => Ok("sing-box-1.11.3-windows-arm64.zip"),
        ("macos", "x86_64") => Ok("sing-box-1.11.3-darwin-amd64.tar.gz"),
        ("macos", "aarch64") => Ok("sing-box-1.11.3-darwin-arm64.tar.gz"),
        ("linux", "x86_64") => Ok("sing-box-1.11.3-linux-amd64.tar.gz"),
        ("linux", "aarch64") => Ok("sing-box-1.11.3-linux-arm64.tar.gz"),
        (os, arch) => bail!("不支持的平台：{os}-{arch}，请手动放置 sing-box 内核"),
    }
}

/// 下载候选地址：官方源 + 常见 GitHub 镜像；可用 PRISM_KERNEL_MIRROR 覆盖镜像前缀
fn candidate_urls(asset: &str) -> Vec<String> {
    let official = format!(
        "https://github.com/SagerNet/sing-box/releases/download/v{KERNEL_VERSION}/{asset}"
    );
    let mut urls = vec![official.clone()];
    let mirrors = [
        "https://ghfast.top/",
        "https://gh-proxy.com/",
        "https://mirror.ghproxy.com/",
    ];
    for prefix in mirrors {
        urls.push(format!("{prefix}{official}"));
    }
    if let Ok(custom) = std::env::var("PRISM_KERNEL_MIRROR") {
        if !custom.is_empty() {
            urls.insert(1, format!("{custom}{official}"));
        }
    }
    urls
}

/// 确保内核二进制就位，返回其路径
pub async fn ensure(app: &AppHandle, work_dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(work_dir).context("创建内核目录失败")?;
    let target = kernel_binary_path(work_dir);
    if target.is_file() {
        emit(app, "ready", 100, "内核已存在");
        return Ok(target);
    }

    let asset = asset_name()?;
    let urls = candidate_urls(asset);

    // 临时下载/解压目录（用纳秒后缀避免并发冲突）
    let stamp = UNIX_EPOCH
        .elapsed()
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let stage_dir = work_dir.join(format!("kernel-stage-{stamp}"));
    std::fs::create_dir_all(&stage_dir).context("创建临时目录失败")?;
    let archive_path = stage_dir.join(asset);

    // 依次尝试各下载源
    let mut last_err: Option<anyhow::Error> = None;
    let mut downloaded = false;

    for url in &urls {
        emit(app, "download", 0, format!("正在下载内核（{KERNEL_VERSION}）…"));
        tracing::info!("downloading sing-box kernel from {url}");

        match download_to_file(app, url, &archive_path).await {
            Ok(()) => {
                downloaded = true;
                break;
            }
            Err(e) => {
                tracing::warn!("kernel download source failed ({url}): {e}");
                emit(app, "download", 0, format!("下载源失败，正在切换镜像…"));
                last_err = Some(e);
            }
        }
    }

    if !downloaded {
        let _ = std::fs::remove_dir_all(&stage_dir);
        let e = last_err.unwrap_or_else(|| anyhow::anyhow!("没有可用的下载源"));
        emit(app, "error", 0, "内核下载失败");
        bail!("sing-box 内核下载失败（已尝试官方源与镜像）：{e}");
    }

    // 解压
    emit(app, "extract", 95, "正在解压内核…");
    extract_archive(&archive_path, &stage_dir).await?;

    // 解压包内有一层 sing-box-<ver>-<os>-<arch>/ 目录，递归找到二进制
    let found = find_binary(&stage_dir).context("压缩包内未找到 sing-box 二进制")?;

    // 落盘到目标路径（同分区 rename 是原子操作）
    let _ = std::fs::remove_file(&target);
    std::fs::rename(&found, &target).or_else(|_| {
        // 跨设备回退：复制 + 删除
        std::fs::copy(&found, &target)?;
        std::fs::remove_file(&found).ok();
        anyhow::Ok(())
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755));
    }

    let _ = std::fs::remove_dir_all(&stage_dir);

    if !target.is_file() {
        emit(app, "error", 0, "内核写入失败");
        bail!("内核二进制写入失败：{}", target.display());
    }

    emit(app, "ready", 100, format!("sing-box {KERNEL_VERSION} 就绪"));
    tracing::info!("sing-box kernel installed at {}", target.display());
    Ok(target)
}

/// 流式下载并按 ~250ms 节流推送百分比
async fn download_to_file(app: &AppHandle, url: &str, dest: &Path) -> Result<()> {
    // no_proxy：内核运行时系统代理指向本进程，经代理下载会形成自回环
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .no_proxy()
        .build()?;
    let resp = client.get(url).send().await?.error_for_status()?;

    let total = resp.content_length();
    let mut stream = resp.bytes_stream();
    let mut file = tokio::fs::File::create(dest).await?;
    let mut got: u64 = 0;
    let mut last_emit = UNIX_EPOCH.elapsed().map(|d| d.as_millis()).unwrap_or(0);

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        got += chunk.len() as u64;

        let now = UNIX_EPOCH.elapsed().map(|d| d.as_millis()).unwrap_or(0);
        if now - last_emit >= 250 {
            let percent = match total {
                Some(t) if t > 0 => ((got * 100 / t).min(99)) as u8,
                _ => 0,
            };
            emit(
                app,
                "download",
                percent,
                format!("正在下载内核… {:.1} MB", got as f64 / 1_048_576.0),
            );
            last_emit = now;
        }
    }
    file.flush().await?;
    Ok(())
}

/// 解压 zip/tar.gz：优先系统 tar（Windows 10+ 自带 bsdtar，同时支持 zip/tar.gz），
/// Windows 回退 PowerShell Expand-Archive
async fn extract_archive(archive: &Path, dest: &Path) -> Result<()> {
    let output = tokio::process::Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => return Ok(()),
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            tracing::warn!("tar extract failed: {err}");
        }
        Err(e) => {
            tracing::warn!("tar not available: {e}");
        }
    }

    #[cfg(windows)]
    {
        if archive.extension().is_some_and(|ext| ext == "zip") {
            let ps = tokio::process::Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "Expand-Archive -LiteralPath $args[0] -DestinationPath $args[1] -Force",
                    "--",
                ])
                .arg(archive)
                .arg(dest)
                .output()
                .await?;
            if ps.status.success() {
                return Ok(());
            }
            bail!("解压内核压缩包失败（tar/PowerShell 均失败）");
        }
    }

    bail!("解压内核压缩包失败：系统缺少可用的 tar 工具");
}

/// 递归查找名为 sing-box[.exe] 的文件
fn find_binary(root: &Path) -> Result<PathBuf> {
    let want = if cfg!(windows) {
        "sing-box.exe"
    } else {
        "sing-box"
    };
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|n| n == want) {
                return Ok(path);
            }
        }
    }
    bail!("未在解压目录中找到 {want}");
}
