//! Tauri commands，供前端通过 invoke() 调用

use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::core::kernel::{KernelConfig, KernelHandle, generate_secret, probe_version};
use crate::core::state::{AppState, CoreStatus, RunMode, UserSettings, is_port_free, pick_api_addr};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreStatusDto {
    pub status: CoreStatus,
    pub mode: RunMode,
    pub uptime_secs: u64,
}

/// 内核信息（设置页 / 下载向导用）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KernelInfoDto {
    pub exists: bool,
    pub path: Option<String>,
    pub version: Option<String>,
}

#[tauri::command]
pub async fn start_core(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    state
        .transition(CoreStatus::Starting)
        .map_err(|e| e.to_string())?;

    // 启动失败时统一回滚，避免半成品状态
    if let Err(e) = do_start(&app, &state).await {
        let _ = do_stop(&state).await;
        let _ = state.transition(CoreStatus::Error);
        return Err(e);
    }

    *state.started_at.write() = Some(std::time::Instant::now());
    state
        .transition(CoreStatus::Running)
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn do_start(app: &AppHandle, state: &State<'_, AppState>) -> Result<(), String> {
    let mode = *state.mode.read();
    let settings = state.settings.read().clone();

    // 1. 内核工作目录（profile.yaml + config.json + cache.db + kernel.log）
    let work_dir = state.work_dir();
    std::fs::create_dir_all(&work_dir)
        .map_err(|e| format!("创建内核工作目录失败: {}", e))?;

    // 2. 读取订阅档案
    let profile_path = work_dir.join("profile.yaml");
    let profile_text = std::fs::read_to_string(&profile_path).map_err(|_| {
        format!(
            "尚未导入订阅：请先在“订阅”页添加订阅（期望文件：{}）",
            profile_path.display()
        )
    })?;

    // 3. 确保内核二进制就位：缺失则首启自动下载（进度走 kernel-download://progress）
    let binary_path = match resolve_kernel_path(&work_dir) {
        Ok(path) => path,
        Err(_) => {
            tracing::info!("kernel binary missing, triggering first-run download");
            crate::core::kernel_download::ensure(app, &work_dir)
                .await
                .map_err(|e| format!("内核下载失败: {e}"))?;
            resolve_kernel_path(&work_dir)
                .map_err(|e| format!("下载后仍找不到内核: {e}"))?
        }
    };

    // 4. 端口预检 + clash_api 动态选址/密钥，避免与本机其他代理内核冲突
    let listen_host = if settings.allow_lan { "0.0.0.0" } else { "127.0.0.1" };
    if !is_port_free(listen_host, settings.mixed_port) {
        return Err(format!(
            "混合端口 {} 已被占用，请在“设置”中更换端口后重试",
            settings.mixed_port
        ));
    }

    let api_addr = pick_api_addr();
    let api_secret = generate_secret();
    *state.api_addr.write() = api_addr.clone();
    *state.api_secret.write() = api_secret.clone();
    tracing::info!("clash api listening on {api_addr}");

    // 5. 配置构建：Clash YAML → sing-box runtime JSON
    let built =
        crate::core::config_builder::build(&profile_text, mode, &settings, &work_dir, &api_addr, &api_secret)
            .map_err(|e| e.to_string())?;
    for warning in &built.warnings {
        tracing::warn!("config builder: {}", warning);
    }

    // 6. 启动内核
    let kernel_config = KernelConfig {
        binary_path: binary_path.clone(),
        work_dir: work_dir.clone(),
        api_addr,
        api_secret,
        runtime_config: built.config,
    };

    let handle = KernelHandle::spawn(kernel_config, app.clone())
        .await
        .map_err(|e| format!("内核启动失败: {}", e))?;

    // 探测并缓存版本号供设置页展示（失败不影响启动）
    if let Some(version) = probe_version(&binary_path).await {
        tracing::info!("kernel version: {version}");
        *state.kernel_version.write() = Some(version);
    }

    *state.kernel_handle.write() = Some(Arc::new(handle));

    // 7. TUN 模式下路由由内核 auto_route 接管；系统代理模式按用户设置写入系统代理
    if mode == RunMode::SystemProxy && settings.system_proxy {
        crate::platform::set_system_proxy("127.0.0.1", settings.mixed_port)
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// 解析 sing-box 二进制位置，优先级：
/// 1. 环境变量 PRISM_KERNEL_PATH
/// 2. 内核工作目录内 sing-box[.exe]（首启自动下载落盘位置）
/// 3. 主程序同级目录 sing-box[.exe]
pub(crate) fn resolve_kernel_path(work_dir: &std::path::Path) -> anyhow::Result<std::path::PathBuf> {
    let exe_name = if cfg!(windows) {
        "sing-box.exe"
    } else {
        "sing-box"
    };

    let candidates: Vec<std::path::PathBuf> = match std::env::var("PRISM_KERNEL_PATH") {
        Ok(path) => vec![path.into()],
        Err(_) => {
            let mut list = vec![
                crate::core::kernel_download::kernel_binary_path(work_dir),
            ];
            if let Ok(exe) = std::env::current_exe()
                && let Some(dir) = exe.parent()
            {
                list.push(dir.join(exe_name));
            }
            list
        }
    };

    for path in candidates {
        if path.is_file() {
            tracing::info!("using kernel binary: {}", path.display());
            return Ok(path);
        }
    }

    anyhow::bail!("未找到 sing-box 内核二进制")
}

#[tauri::command]
pub async fn stop_core(state: State<'_, AppState>) -> Result<(), String> {
    // 幂等：停止需要等待内核退出（最长 5 秒），期间重复点击/退出清理
    // 会再次进入本函数。transition 内部持写锁，并发下只有一个调用能
    // 完成 Running -> Stopping，其余落入此分支直接忽略
    if let Err(e) = state.transition(CoreStatus::Stopping) {
        let current = *state.status.read();
        if matches!(current, CoreStatus::Stopped | CoreStatus::Stopping) {
            return Ok(());
        }
        return Err(e.to_string());
    }

    if let Err(e) = do_stop(&state).await {
        // 停止失败不能留在 Stopping（没有出边，会永久卡死），转入 Error 允许重试
        let _ = state.transition(CoreStatus::Error);
        return Err(e.to_string());
    }
    *state.started_at.write() = None;

    state
        .transition(CoreStatus::Stopped)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 停止/退出时统一调用：回收 TUN 守卫 → 停内核 → 清系统代理
pub(crate) async fn do_stop(state: &AppState) -> anyhow::Result<()> {
    // 1. 关闭 TUN 守卫（手动路由/DNS 回滚，通常为 None）
    // 先在独立语句完成 write()，避免临时写守卫跨越 await（parking_lot 守卫 !Send）
    let tun_taken = state.tun_guard.write().take();
    if let Some(mut guard) = tun_taken {
        guard.disable().await?;
    }

    // 2. 停止内核
    let handle = state.kernel_handle.write().take();
    if let Some(handle) = handle {
        handle.shutdown().await?;
    }

    // 3. 兜底清除系统代理（无论何种模式都执行，防止残留断网）
    crate::platform::clear_system_proxy()?;

    Ok(())
}

#[tauri::command]
pub async fn get_core_status(state: State<'_, AppState>) -> Result<CoreStatusDto, String> {
    let uptime_secs = state
        .started_at
        .read()
        .map(|t| t.elapsed().as_secs())
        .unwrap_or(0);

    Ok(CoreStatusDto {
        status: *state.status.read(),
        mode: *state.mode.read(),
        uptime_secs,
    })
}

#[tauri::command]
pub async fn set_mode(mode: RunMode, state: State<'_, AppState>) -> Result<(), String> {
    *state.mode.write() = mode;
    // TODO: 运行中切换模式需重载内核（当前在下次启动生效）
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UrlTestRequest {
    pub group: String,
    pub url: Option<String>,
    pub timeout_ms: Option<u32>,
}

#[tauri::command]
pub async fn url_test(
    req: UrlTestRequest,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    state
        .kernel_api()
        .url_test(&req.group, req.url.as_deref(), req.timeout_ms)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn select_proxy(
    group: String,
    name: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .kernel_api()
        .select_proxy(&group, &name)
        .await
        .map_err(|e| e.to_string())
}

/// 下载订阅并写入 profile.yaml。
/// 兼容两种返回：Clash YAML 明文、base64 包裹的内容。
#[tauri::command]
pub async fn update_subscription(
    url: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let work_dir = state.work_dir();
    std::fs::create_dir_all(&work_dir).map_err(|e| e.to_string())?;

    // 伪装为 Clash 客户端，多数机场据此返回 Clash YAML；
    // 必须 no_proxy：内核运行时系统代理指向本进程，走代理会形成自回环
    let client = reqwest::Client::builder()
        .user_agent("clash-verge/v2.0.0")
        .timeout(std::time::Duration::from_secs(30))
        .no_proxy()
        .build()
        .map_err(|e| e.to_string())?;

    let bytes = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("下载订阅失败: {}", e))?
        .bytes()
        .await
        .map_err(|e| format!("读取订阅响应失败: {}", e))?;

    let mut text = String::from_utf8(bytes.to_vec())
        .map_err(|_| "订阅内容不是有效 UTF-8 文本".to_string())?;

    // base64 包裹：明文不含 YAML/JSON 结构时尝试解码
    if !text.contains("proxies") && !text.trim_start().starts_with('{') {
        if let Ok(decoded) = try_decode_base64(&text) {
            text = decoded;
        }
    }

    // URI 列表订阅（ss://、vmess://）暂不支持，明确报错而非静默损坏
    if text.lines().any(|l| {
        let l = l.trim();
        l.starts_with("ss://") || l.starts_with("vmess://") || l.starts_with("vless://")
    }) {
        return Err("该订阅为 URI 列表格式（ss:// vmess://），当前版本暂不支持，请更换为 Clash YAML 订阅".into());
    }

    if !text.contains("proxies") && !text.contains("{") {
        return Err("订阅内容无法识别：既不是 Clash YAML，也不是 sing-box JSON".into());
    }

    let profile_path = work_dir.join("profile.yaml");
    std::fs::write(&profile_path, &text)
        .map_err(|e| format!("写入订阅文件失败: {}", e))?;
    tracing::info!("subscription saved to {}", profile_path.display());

    // 持久化订阅记录（规则页回填 URL）
    crate::core::store::upsert_subscription(&state.data_dir, &url)
        .map_err(|e| format!("保存订阅记录失败: {e}"))?;

    // 内核运行中：后续应触发热重载，当前需要重启内核生效
    if *state.status.read() == CoreStatus::Running {
        tracing::info!("subscription updated; restart core to take effect (hot reload TODO)");
    }

    Ok(())
}

/// 尝试 base64 解码（容错换行/空白），成功且结果为可读 UTF-8 时返回
fn try_decode_base64(input: &str) -> Result<String, String> {
    let compact: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    let decoded = STANDARD
        .decode(&compact)
        .map_err(|e| format!("not base64: {}", e))?;
    String::from_utf8(decoded).map_err(|e| format!("decoded bytes not UTF-8: {}", e))
}

/// 订阅记录（前端展示用）：在存储记录上附加 是否活跃 / 节点数
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionDto {
    pub url: String,
    pub updated_at: i64,
    pub name: Option<String>,
    /// 是否为当前生效订阅（最近更新的一条）
    pub active: bool,
    /// 节点数：仅活跃订阅有值（解析 work_dir/profile.yaml 统计 proxies 条目）
    pub node_count: Option<usize>,
}

/// 获取已保存的订阅记录（订阅页展示）
#[tauri::command]
pub async fn list_subscriptions(
    state: State<'_, AppState>,
) -> Result<Vec<SubscriptionDto>, String> {
    let records = crate::core::store::load_subscriptions(&state.data_dir);
    // 单订阅模型：最新 updated_at 即当前生效订阅
    let active_url = records
        .iter()
        .max_by_key(|r| r.updated_at)
        .map(|r| r.url.clone());

    let node_count = count_profile_nodes(&state.work_dir());

    Ok(records
        .into_iter()
        .map(|r| {
            let active = Some(&r.url) == active_url.as_ref();
            SubscriptionDto {
                url: r.url,
                updated_at: r.updated_at,
                name: r.name,
                active,
                node_count: if active { node_count } else { None },
            }
        })
        .collect())
}

/// 统计当前 profile.yaml 的 proxies 条目数；文件缺失/解析失败返回 None
fn count_profile_nodes(work_dir: &std::path::Path) -> Option<usize> {
    let text = std::fs::read_to_string(work_dir.join("profile.yaml")).ok()?;
    let value: serde_yaml::Value = serde_yaml::from_str(&text).ok()?;
    value.get("proxies")?.as_sequence().map(|s| s.len())
}

/// 删除一条订阅记录
#[tauri::command]
pub async fn delete_subscription(
    url: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    crate::core::store::delete_subscription(&state.data_dir, &url)
        .map_err(|e| format!("删除订阅失败: {e}"))
}

/// 激活 Pro 高级功能（占位实现）。
// TODO(商业化)：当前仅校验激活码格式并持久化标记；
// 正式上线前替换为真实校验（服务端签名 / StoreKit / Google Play Billing）。
#[tauri::command]
pub async fn activate_pro(code: String, state: State<'_, AppState>) -> Result<bool, String> {
    if !is_valid_pro_code(&code) {
        return Ok(false);
    }
    let mut settings = state.settings.read().clone();
    settings.pro_unlocked = true;
    state
        .save_settings(settings)
        .map_err(|e| format!("保存激活状态失败: {e}"))?;
    tracing::info!("pro activated (placeholder validation)");
    Ok(true)
}

/// 占位激活码格式：PRISM-XXXX-XXXX-XXXX（每段 4 位字母数字）
fn is_valid_pro_code(code: &str) -> bool {
    let parts: Vec<&str> = code.split('-').collect();
    parts.len() == 4
        && parts[0] == "PRISM"
        && parts[1..]
            .iter()
            .all(|p| p.len() == 4 && p.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// 查询内核是否就位（不触发下载）
#[tauri::command]
pub async fn get_kernel_info(state: State<'_, AppState>) -> Result<KernelInfoDto, String> {
    Ok(kernel_info(&state))
}

/// 手动触发内核下载/修复（设置页按钮）
#[tauri::command]
pub async fn ensure_kernel(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<KernelInfoDto, String> {
    let work_dir = state.work_dir();
    let path = match resolve_kernel_path(&work_dir) {
        Ok(path) => path,
        Err(_) => crate::core::kernel_download::ensure(&app, &work_dir)
            .await
            .map_err(|e| format!("内核下载失败: {e}"))?,
    };

    if let Some(version) = probe_version(&path).await {
        *state.kernel_version.write() = Some(version);
    }
    Ok(kernel_info(&state))
}

fn kernel_info(state: &AppState) -> KernelInfoDto {
    match resolve_kernel_path(&state.work_dir()) {
        Ok(path) => KernelInfoDto {
            exists: true,
            path: Some(path.to_string_lossy().to_string()),
            version: state.kernel_version.read().clone(),
        },
        Err(_) => KernelInfoDto {
            exists: false,
            path: None,
            version: None,
        },
    }
}

#[tauri::command]
pub async fn get_connections(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    state
        .kernel_api()
        .get_connections()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_traffic_stats(
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    // 实时流量走 WebSocket 事件（traffic://tick），此处仅返回快照兜底
    state
        .kernel_api()
        .get_connections()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_rules(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    state.kernel_api().get_rules().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_proxy_groups(
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    state
        .kernel_api()
        .get_proxies()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<UserSettings, String> {
    Ok(state.settings.read().clone())
}

#[tauri::command]
pub async fn save_settings(
    settings: UserSettings,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // 自启动立即生效
    crate::platform::set_auto_start(settings.auto_start).map_err(|e| e.to_string())?;

    // 落盘 settings.json，重启后恢复
    state
        .save_settings(settings)
        .map_err(|e| format!("保存设置失败: {e}"))?;
    // TODO: 端口等变更在内核运行时需热重载
    Ok(())
}
