//! Go 代理内核封装
//!
//! 通信策略：将 sing-box 编译为独立 sidecar 二进制，Rust 通过本地 HTTP API
//! (默认 127.0.0.1:9090，占用时动态选择) 与其通信，并订阅其 WebSocket 数据流。
//!
//! 注意：sing-box 只接受 **JSON** 配置，运行时配置必须写入 config.json。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
#[cfg(not(target_os = "ios"))]
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
#[cfg(not(target_os = "ios"))]
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
#[cfg(not(target_os = "ios"))]
use tokio::process::Command;
use tokio::task::JoinHandle;

/// 首选 external-controller 地址；被占用时会退回到系统分配的随机端口
pub const PREFERRED_API_ADDR: &str = "127.0.0.1:9090";

/// iOS 内嵌内核版本（与 ios-kernel/go.mod 的 sing-box 依赖保持一致）
#[cfg(target_os = "ios")]
pub const IOS_EMBEDDED_VERSION: &str = "1.11.3";

/// iOS 内嵌内核 FFI（sing-box c-archive，见 src-tauri/ios-kernel/main.go）。
/// 进程内运行：无子进程、无 pid；clash_api 与桌面 sidecar 完全一致。
#[cfg(target_os = "ios")]
mod ios_ffi {
    use std::ffi::{CStr, CString};
    use std::os::raw::c_char;
    use std::path::Path;

    extern "C" {
        fn PrismKernelStart(config_path: *const c_char, work_dir: *const c_char) -> *mut c_char;
        fn PrismKernelCheck(config_path: *const c_char, work_dir: *const c_char) -> *mut c_char;
        fn PrismKernelStop();
        fn PrismKernelFree(p: *mut c_char);
    }

    pub fn start(config_path: &Path, work_dir: &Path) -> Result<(), String> {
        let cp = CString::new(config_path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        let wd = CString::new(work_dir.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        unsafe {
            let err = PrismKernelStart(cp.as_ptr(), wd.as_ptr());
            if !err.is_null() {
                let msg = CStr::from_ptr(err).to_string_lossy().into_owned();
                PrismKernelFree(err);
                return Err(msg);
            }
        }
        Ok(())
    }

    pub fn check(config_path: &Path, work_dir: &Path) -> Result<(), String> {
        let cp = CString::new(config_path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        let wd = CString::new(work_dir.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        unsafe {
            let err = PrismKernelCheck(cp.as_ptr(), wd.as_ptr());
            if !err.is_null() {
                let msg = CStr::from_ptr(err).to_string_lossy().into_owned();
                PrismKernelFree(err);
                return Err(msg);
            }
        }
        Ok(())
    }

    pub fn stop() {
        unsafe { PrismKernelStop() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KernelConfig {
    /// sidecar 内核二进制路径
    pub binary_path: PathBuf,
    /// 工作目录（存放 config.json / 缓存 / kernel.log）
    pub work_dir: PathBuf,
    /// external-controller 监听地址
    pub api_addr: String,
    /// clash_api 鉴权密钥（写入配置 + 所有请求/WS 带 Bearer 头）
    pub api_secret: String,
    /// 生成的 sing-box 运行时配置（JSON）
    pub runtime_config: serde_json::Value,
}

pub struct KernelHandle {
    pub config: KernelConfig,
    /// sidecar 子进程句柄（停止时取出并等待退出）
    child: Mutex<Option<Child>>,
    /// 子进程 pid
    pub pid: u32,
    /// 内核事件流任务（停止内核时 abort，避免重连任务泄漏导致事件翻倍）
    stream_tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl KernelHandle {
    /// 启动内核 sidecar 进程
    pub async fn spawn(config: KernelConfig, app_handle: AppHandle) -> Result<Self> {
        // 1. 写入运行时配置（sing-box 只解析 JSON）
        std::fs::create_dir_all(&config.work_dir)
            .context("failed to create kernel work dir")?;
        let config_path = config.work_dir.join("config.json");
        let config_bytes = serde_json::to_vec_pretty(&config.runtime_config)
            .context("failed to serialize kernel config")?;
        std::fs::write(&config_path, config_bytes)
            .context("failed to write kernel config.json")?;

        // 2. 清空旧内核日志，接管子进程 stdout/stderr（Go 日志默认写 stderr）
        let log_path = config.work_dir.join("kernel.log");
        std::fs::write(&log_path, b"").ok();

        // 2.5 确保 geoip.db / geosite.db 就位：legacy geoip/geosite 规则需要本地
        // 数据库，缺失时 sing-box 会走代理出站下载（启动期代理未就绪，必失败）。
        // 数据库随包分发（见 scripts/fetch-kernel.cjs），这里从候选位置拷贝到工作目录
        ensure_geo_databases(&config, &app_handle);

        // 3. 启动内核：iOS 进程内嵌（c-archive FFI），其他平台 spawn sidecar
        // ENABLE_DEPRECATED_GEOIP/GEOSITE：Clash 订阅常见 GEOIP/GEOSITE 规则，
        // 目前转换为 sing-box legacy geoip/geosite 字段；1.11 起需显式环境变量启用。
        // TODO(内核升级)：迁移到 rule_set（.srs 规则集）后移除这两个环境变量
        #[cfg(target_os = "ios")]
        let (child, pid): (Option<Child>, u32) = {
            ios_ffi::start(&config_path, &config.work_dir)
                .map_err(|e| anyhow::anyhow!("内嵌内核启动失败: {e}"))?;
            tracing::info!("sing-box embedded kernel started in-process");
            (None, std::process::id())
        };
        #[cfg(not(target_os = "ios"))]
        let (child, pid): (Option<Child>, u32) = {
            let mut child = Command::new(&config.binary_path)
                .arg("run")
                .arg("-c")
                .arg(&config_path)
                .env("ENABLE_DEPRECATED_GEOIP", "true")
                .env("ENABLE_DEPRECATED_GEOSITE", "true")
                // 固定工作目录：sing-box 在此读写 geoip.db / geosite.db / cache.db
                .current_dir(&config.work_dir)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .context("failed to spawn sing-box process")?;

            let pid = child
                .id()
                .context("sing-box process exited immediately (no pid)")?;
            tracing::info!("sing-box process started, PID: {}", pid);
            register_kernel_pid(pid);

            spawn_log_pipe(
                child.stdout.take().context("missing child stdout")?,
                "out",
                &log_path,
            );
            spawn_log_pipe(
                child.stderr.take().context("missing child stderr")?,
                "err",
                &log_path,
            );
            (Some(child), pid)
        };

        // 4. 轮询 API 直到就绪（最长 15 秒）
        let api = KernelApi::new(&config.api_addr, &config.api_secret);
        let mut ready = false;

        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(500)).await;

            if let Ok(value) = api.get_version().await {
                tracing::info!("sing-box ready: {}", value);
                ready = true;
                break;
            }
        }

        if !ready {
            // 启动失败则回收内核并落盘错误上下文，避免残留
            #[cfg(target_os = "ios")]
            ios_ffi::stop();
            #[cfg(not(target_os = "ios"))]
            {
                if let Some(mut c) = child {
                    let _ = c.start_kill();
                    let _ = c.wait().await;
                }
                clear_kernel_pid(pid);
            }
            let tail = read_log_tail(&log_path);
            anyhow::bail!(
                "sing-box 在 15 秒内未就绪，请查看内核日志 {} ；末尾输出：{}",
                log_path.display(),
                tail
            );
        }

        // 5. 订阅内核 WebSocket 数据流（traffic / connections / memory）
        let stream_tasks = super::events::spawn_all(
            &config.api_addr,
            &config.api_secret,
            app_handle,
        );

        Ok(Self {
            config,
            child: Mutex::new(child),
            pid,
            stream_tasks: Mutex::new(stream_tasks),
        })
    }

    /// 优雅停止内核
    pub async fn shutdown(&self) -> Result<()> {
        tracing::info!("shutting down sing-box process {}", self.pid);

        // 先终止事件流任务，防止 stop/start 后订阅者翻倍
        // 锁守卫必须在 await 前释放（parking_lot 守卫不是 Send）
        {
            let mut tasks = self.stream_tasks.lock();
            for task in tasks.drain(..) {
                task.abort();
            }
        }

        let child = self.child.lock().take();
        if let Some(mut child) = child {
            // Unix 先 SIGTERM 给 5 秒优雅退出；Windows 直接 TerminateProcess
            #[cfg(unix)]
            unsafe {
                libc::kill(self.pid as i32, libc::SIGTERM);
            }

            let exited = tokio::time::timeout(Duration::from_secs(5), child.wait())
                .await
                .is_ok_and(|r| r.is_ok());

            if !exited {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
        }

        #[cfg(target_os = "ios")]
        ios_ffi::stop();

        clear_kernel_pid(self.pid);
        tracing::info!("sing-box process {} stopped", self.pid);
        Ok(())
    }
}

/// 静态校验待应用的配置：sing-box check -c <path>。
/// 重启前调用——校验失败时旧内核保留，网络不中断。
/// iOS 上无法 spawn 子进程，改为调用内嵌 FFI 的 PrismKernelCheck（解析+构建后关闭）。
pub async fn check_config(
    binary: &std::path::Path,
    work_dir: &std::path::Path,
    config_path: &std::path::Path,
) -> Result<()> {
    #[cfg(target_os = "ios")]
    {
        let _ = binary;
        ios_ffi::check(config_path, work_dir)
            .map_err(|e| anyhow::anyhow!("新配置校验未通过，已保留当前内核：\n{}", e))?;
    }
    #[cfg(not(target_os = "ios"))]
    {
        let output = Command::new(binary)
            .arg("check")
            .arg("-c")
            .arg(config_path)
            // 与 spawn 一致：legacy GEOIP/GEOSITE 规则需要环境变量启用
            .env("ENABLE_DEPRECATED_GEOIP", "true")
            .env("ENABLE_DEPRECATED_GEOSITE", "true")
            .current_dir(work_dir)
            .output()
            .await
            .context("failed to run sing-box check")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // 只保留末尾 800 字符，避免前端 toast 被刷屏
            let chars: Vec<char> = stderr.chars().collect();
            let skip = chars.len().saturating_sub(800);
            let tail: String = chars[skip..].iter().collect();
            anyhow::bail!("新配置校验未通过，已保留当前内核：\n{}", tail.trim());
        }
    }
    Ok(())
}

/// 生成随机 clash_api secret（本地环回用途，基于时间+pid 两次哈希拼成 32 hex）
pub fn generate_secret() -> String {
    fn hash_seed(seed: u64) -> u64 {
        let mut h = DefaultHasher::new();
        UNIX_EPOCH.elapsed().map(|d| d.as_nanos()).unwrap_or(0).hash(&mut h);
        std::process::id().hash(&mut h);
        seed.hash(&mut h);
        h.finish()
    }
    format!("{:016x}{:016x}", hash_seed(1), hash_seed(2))
}

/// 执行 `<binary> version` 并提取版本号（sing-box 输出 JSON）。
/// iOS 无二进制可执行，直接返回内嵌内核版本。
pub async fn probe_version(binary: &std::path::Path) -> Option<String> {
    #[cfg(target_os = "ios")]
    {
        let _ = binary;
        return Some(format!("{} （内置）", IOS_EMBEDDED_VERSION));
    }
    #[cfg(not(target_os = "ios"))]
    {
        let output = Command::new(binary).arg("version").output().await.ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        serde_json::from_str::<serde_json::Value>(text.trim())
            .ok()
            .and_then(|v| v.get("version").and_then(|x| x.as_str().map(str::to_string)))
    }
}

/// 读取内核日志末尾最多 2KB，用于启动失败提示
fn read_log_tail(path: &std::path::Path) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return "<无日志>".into();
    };
    let start = bytes.len().saturating_sub(2048);
    String::from_utf8_lossy(&bytes[start..]).replace('\n', " | ")
}

/// 将随包分发的 geoip.db / geosite.db 拷贝到内核工作目录（已存在则跳过）。
/// 候选来源：内核二进制同级（sidecar 位置）、应用资源目录、源码树 binaries（dev）。
fn ensure_geo_databases(config: &KernelConfig, app_handle: &AppHandle) {
    const DBS: [&str; 2] = ["geoip.db", "geosite.db"];

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = config.binary_path.parent() {
        candidates.push(dir.to_path_buf());
    }
    if let Ok(dir) = app_handle.path().resource_dir() {
        candidates.push(dir);
    }
    // dev：内核二进制在 target/debug，数据库在源码树 src-tauri/binaries
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries"));

    for db in DBS {
        let dest = config.work_dir.join(db);
        if dest.is_file() {
            continue;
        }
        for dir in &candidates {
            let src = dir.join(db);
            if src.is_file() {
                match std::fs::copy(&src, &dest) {
                    Ok(_) => {
                        tracing::info!("copied {} -> {}", src.display(), dest.display());
                        break;
                    }
                    Err(e) => tracing::warn!("copy {} failed: {e}", src.display()),
                }
            }
        }
        if !dest.is_file() {
            tracing::warn!(
                "{db} not found in any candidate location; kernel will try runtime download (likely fails)"
            );
        }
    }
}

/// 将子进程的一条输出管道按行写入 kernel.log，同时转发到 tracing
/// （iOS 内核进程内运行，日志由 Go 侧直接写文件，无管道可接）
#[cfg(not(target_os = "ios"))]
fn spawn_log_pipe<R>(reader: R, tag: &'static str, log_path: &std::path::Path)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let log_path = log_path.to_path_buf();
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::info!(target: "kernel", "[{tag}] {line}");
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
            {
                let ts = chrono_like_timestamp();
                let _ = writeln!(f, "[{ts}] [{tag}] {line}");
            }
        }
    });
}

/// 简易 UTC 时间戳（不引入 chrono 依赖，仅用于内核日志排障）
#[cfg(not(target_os = "ios"))]
fn chrono_like_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let day_secs = secs % 86400;
    let (h, m, s) = (day_secs / 3600, day_secs % 3600 / 60, day_secs % 60);
    format!("{h:02}:{m:02}:{s:02}")
}

// ---------------- 全局 PID（panic hook 兜底强杀） ----------------

static KERNEL_PID: OnceLock<Mutex<Option<u32>>> = OnceLock::new();

fn pid_store() -> &'static Mutex<Option<u32>> {
    KERNEL_PID.get_or_init(|| Mutex::new(None))
}

#[cfg(not(target_os = "ios"))]
fn register_kernel_pid(pid: u32) {
    *pid_store().lock() = Some(pid);
}

fn clear_kernel_pid(pid: u32) {
    let mut guard = pid_store().lock();
    if *guard == Some(pid) {
        *guard = None;
    }
}

/// panic 兜底时使用：获取当前未清理的内核 pid
pub fn current_kernel_pid() -> Option<u32> {
    *pid_store().lock()
}

/// 内核 API 客户端（与 external-controller 通信）
pub struct KernelApi {
    base: String,
    client: reqwest::Client,
}

impl KernelApi {
    pub fn new(api_addr: &str, api_secret: &str) -> Self {
        let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(10));
        if !api_secret.is_empty() {
            let mut headers = reqwest::header::HeaderMap::new();
            if let Ok(value) = reqwest::header::HeaderValue::from_str(&format!("Bearer {api_secret}"))
            {
                headers.insert(reqwest::header::AUTHORIZATION, value);
            }
            builder = builder.default_headers(headers);
        }
        Self {
            base: format!("http://{api_addr}"),
            client: builder.build().expect("failed to build HTTP client"),
        }
    }

    /// 获取版本信息
    pub async fn get_version(&self) -> Result<serde_json::Value> {
        Ok(self
            .client
            .get(format!("{}/version", self.base))
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?)
    }

    /// 获取所有代理节点和策略组
    pub async fn get_proxies(&self) -> Result<serde_json::Value> {
        Ok(self
            .client
            .get(format!("{}/proxies", self.base))
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?)
    }

    /// 切换 Selector 策略组选择
    pub async fn select_proxy(&self, group: &str, name: &str) -> Result<()> {
        let url = format!("{}/proxies/{}", self.base, group);
        let resp = self
            .client
            .put(&url)
            .json(&serde_json::json!({ "name": name }))
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("select proxy failed: HTTP {}", resp.status());
        }
        Ok(())
    }

    /// 策略组整组测速：GET /group/{name}/delay
    /// （/proxies/{name}/delay 只支持单节点，对策略组调用会返回 400）
    /// 成功响应会将每个成员的延迟写入节点 history，前端据此展示
    pub async fn group_url_test(
        &self,
        name: &str,
        url: Option<&str>,
        timeout_ms: Option<u32>,
    ) -> Result<serde_json::Value> {
        let endpoint = format!("{}/group/{}/delay", self.base, name);
        // sing-box 要求显式携带 url 和 timeout 参数，缺省会直接 400
        let request = self.client.get(&endpoint).query(&[
            ("url", url.unwrap_or("http://www.gstatic.com/generate_204").to_string()),
            ("timeout", timeout_ms.unwrap_or(5000).to_string()),
        ]);
        let resp = request.send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("group url test failed: HTTP {}", resp.status());
        }
        Ok(resp.json::<serde_json::Value>().await?)
    }

    /// 获取当前规则列表
    pub async fn get_rules(&self) -> Result<serde_json::Value> {
        Ok(self
            .client
            .get(format!("{}/rules", self.base))
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?)
    }

    /// 获取实时连接快照
    pub async fn get_connections(&self) -> Result<serde_json::Value> {
        Ok(self
            .client
            .get(format!("{}/connections", self.base))
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?)
    }

    /// 关闭单条连接：DELETE /connections/{id}
    /// （sing-box 连接 id 为十六进制字符串，可直接拼路径）
    pub async fn close_connection(&self, id: &str) -> Result<()> {
        let resp = self
            .client
            .delete(format!("{}/connections/{}", self.base, id))
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("close connection failed: HTTP {}", resp.status());
        }
        Ok(())
    }

    /// 关闭全部连接：DELETE /connections
    pub async fn close_all_connections(&self) -> Result<()> {
        let resp = self
            .client
            .delete(format!("{}/connections", self.base))
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("close all connections failed: HTTP {}", resp.status());
        }
        Ok(())
    }
}
