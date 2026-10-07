//! Tauri commands，供前端通过 invoke() 调用

use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::core::kernel::{
    KernelApi, KernelConfig, KernelHandle, check_config, generate_secret, probe_version,
};
use crate::core::license::EntitlementStatus;
use crate::core::state::{AppState, CoreStatus, RunMode, UserSettings, is_port_free, pick_api_addr};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreStatusDto {
    pub status: CoreStatus,
    pub mode: RunMode,
    pub uptime_secs: u64,
    /// 主选择组（route.final 链上最深的 selector；内核未运行时为 None）
    pub main_selector: Option<String>,
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

    // 启动后自动测速并切换到延迟最低的可用节点（用户随后仍可手动改选）
    if let Some(group) = state.main_selector.read().clone() {
        spawn_auto_select(
            app.clone(),
            state.api_addr.read().clone(),
            state.api_secret.read().clone(),
            group,
        );
    }
    Ok(())
}

/// 启动准备结果：校验通过但尚未停机/启动（restart 时先产出它再做静态校验）
struct PreparedStart {
    settings: UserSettings,
    work_dir: std::path::PathBuf,
    api_addr: String,
    api_secret: String,
    built: crate::core::config_builder::BuiltConfig,
    binary_path: std::path::PathBuf,
}

/// 启动前的全部准备与校验（不提权变更、不写内核状态）：
/// 提权预检 → 工作目录 → 订阅 profile → 内核二进制 → 端口预检 → 构建配置
async fn prepare_start(
    app: &AppHandle,
    state: &State<'_, AppState>,
) -> Result<PreparedStart, String> {
    let mode = *state.mode.read();
    let settings = state.settings.read().clone();

    // TUN 提权预检：内核要创建虚拟网卡，非管理员/root 时 sing-box tun inbound
    // 必然失败，且错误表现为"15 秒未就绪"，提前给出可操作的明确错误
    #[cfg(target_os = "ios")]
    if mode == RunMode::Tun {
        return Err(
            "iOS 版暂不支持 TUN 模式（系统级 VPN 需 NetworkExtension，后续版本提供），请使用代理模式"
                .into(),
        );
    }
    #[cfg(not(target_os = "ios"))]
    if mode == RunMode::Tun && !crate::platform::has_elevated_privilege() {
        return Err(
            "TUN 模式需要管理员/root 权限：请完全退出 Prism 后以管理员身份重新运行（macOS/Linux 使用 sudo）"
                .into(),
        );
    }

    let work_dir = state.work_dir();
    std::fs::create_dir_all(&work_dir)
        .map_err(|e| format!("创建内核工作目录失败: {}", e))?;

    // 读取全部启用订阅的 profile（多订阅模型；旧版单 profile.yaml 自动兼容）
    let profiles = load_enabled_profiles(state);
    if profiles.is_empty() {
        let has_records =
            !crate::core::store::load_subscriptions(&state.data_dir).is_empty();
        return Err(if has_records {
            "订阅内容缺失（本地缓存不存在）：请在「订阅」页点击更新，成功后再启动内核".into()
        } else {
            "尚未导入订阅：请先在「订阅」页添加订阅".into()
        });
    }

    // 确保内核二进制就位：缺失则首启自动下载（进度走 kernel-download://progress）
    // iOS 内核静态链接进主程序（ios-kernel c-archive），无二进制解析/下载
    #[cfg(not(target_os = "ios"))]
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
    #[cfg(target_os = "ios")]
    let binary_path = std::path::PathBuf::new();

    // 端口预检，避免与本机其他代理内核冲突
    let listen_host = if settings.allow_lan { "0.0.0.0" } else { "127.0.0.1" };
    if !is_port_free(listen_host, settings.mixed_port) {
        return Err(format!(
            "混合端口 {} 已被占用，请在“设置”中更换端口后重试",
            settings.mixed_port
        ));
    }

    // clash_api 动态选址/密钥；仅在本地持有，apply_prepared 时才写入状态
    let api_addr = pick_api_addr();
    let api_secret = generate_secret();

    // 配置构建：自定义规则 + Clash YAML（多订阅合并）→ sing-box runtime JSON
    let custom_rules = crate::core::store::load_custom_rules(&state.data_dir);
    let built = crate::core::config_builder::build(
        &profiles,
        &custom_rules,
        mode,
        &settings,
        &state.data_dir,
        &work_dir,
        &api_addr,
        &api_secret,
    )
    .map_err(|e| e.to_string())?;
    for warning in &built.warnings {
        tracing::warn!("config builder: {}", warning);
    }

    Ok(PreparedStart {
        settings,
        work_dir,
        api_addr,
        api_secret,
        built,
        binary_path,
    })
}

/// 应用已校验的启动准备：写 API 状态 → 启动内核 → 主选择组 → 系统代理
async fn apply_prepared(
    prepared: PreparedStart,
    app: &AppHandle,
    state: &State<'_, AppState>,
) -> Result<(), String> {
    let PreparedStart {
        settings,
        work_dir,
        api_addr,
        api_secret,
        built,
        binary_path,
    } = prepared;

    *state.api_addr.write() = api_addr.clone();
    *state.api_secret.write() = api_secret.clone();
    tracing::info!("clash api listening on {api_addr}");

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

    // 解析主选择组：route.final 沿 selector 链走到最深的选择组。
    // 节点页默认落在该组——GLOBAL 等虚拟组不在路由链路中，在其中选节点不会生效。
    let api = state.kernel_api();
    let main = resolve_main_selector(&api, &built.final_group).await;
    if let Some(group) = &main {
        tracing::info!("main selector group: {group} (final: {})", built.final_group);
    } else {
        tracing::warn!(
            "no main selector resolved from final group '{}'",
            built.final_group
        );
    }
    *state.main_selector.write() = main;

    // TUN 模式下路由由内核 auto_route 接管；系统代理模式按用户设置写入系统代理
    if *state.mode.read() == RunMode::SystemProxy && settings.system_proxy {
        crate::platform::set_system_proxy("127.0.0.1", settings.mixed_port)
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

async fn do_start(app: &AppHandle, state: &State<'_, AppState>) -> Result<(), String> {
    let prepared = prepare_start(app, state).await?;
    apply_prepared(prepared, app, state).await
}

/// 收集启用订阅的 profile 内容；无记录时回退旧版 work_dir/profile.yaml（升级兼容）。
/// 元素：(订阅URL, 展示名, profile内容)
fn load_enabled_profiles(state: &AppState) -> Vec<(String, String, String)> {
    let records = crate::core::store::load_subscriptions(&state.data_dir);
    let enabled: Vec<_> = records.iter().filter(|r| r.enabled).cloned().collect();
    let mut profiles = Vec::new();

    // 旧版（单订阅时代）内容落盘位置，仅用于一次性迁移
    let legacy_candidates = [
        state.data_dir.join("profile.yaml"),
        state.work_dir().join("profile.yaml"),
    ];
    let legacy_text: Option<String> = legacy_candidates
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok());

    for (i, record) in enabled.iter().enumerate() {
        let path = crate::core::store::profile_path(&state.data_dir, &record.url);
        // 已缓存内容必须是含节点的有效 Clash YAML；空壳（如机场返回的
        // JSON 错误页）视为缺失，走旧版迁移兜底恢复
        let cached_valid = std::fs::read_to_string(&path)
            .ok()
            .filter(|text| {
                crate::core::config_builder::validate_subscription_text(text)
                    .map(|_| true)
                    .unwrap_or_else(|e| {
                        tracing::warn!("cached profile invalid for {}: {e}", record.url);
                        false
                    })
            });
        let text = match cached_valid {
            Some(text) => text,
            None => {
                // 新版按 URL 哈希落盘；旧版本内容在 data_dir/profile.yaml 或
                // kernel/profile.yaml——首次升级时迁移到哈希路径，后续逻辑统一
                if let Some(text) = &legacy_text {
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if std::fs::write(&path, text).is_ok() {
                        tracing::info!("migrated legacy profile.yaml -> {}", path.display());
                        text.clone()
                    } else {
                        tracing::warn!("profile missing and legacy migration failed: {}", path.display());
                        continue;
                    }
                } else {
                    tracing::warn!("read profile failed for {}: file missing", record.url);
                    continue;
                }
            }
        };
        let name = record
            .name
            .clone()
            .unwrap_or_else(|| format!("订阅{}", i + 1));
        profiles.push((record.url.clone(), name, text));
    }
    // 无任何启用记录时，旧版单 profile.yaml 仍可兜底
    if profiles.is_empty()
        && let Some(text) = legacy_text
    {
        profiles.push((String::new(), "默认".to_string(), text));
    }
    profiles
}

/// 重建配置并重启内核（调用方保证当前为 Running）。
/// 先构建并静态校验新配置，通过后才停旧内核；校验失败则网络不中断。
async fn restart_core(app: &AppHandle, state: &State<'_, AppState>) -> Result<(), String> {
    // 1. 准备新配置（不改变任何状态）
    let prepared = prepare_start(app, state).await?;

    // 2. sing-box check 静态校验
    let check_path = state.work_dir().join("config.precheck.json");
    let bytes = serde_json::to_vec_pretty(&prepared.built.config)
        .map_err(|e| format!("序列化预检配置失败: {e}"))?;
    std::fs::write(&check_path, bytes).map_err(|e| format!("写入预检配置失败: {e}"))?;
    if let Err(e) = check_config(&prepared.binary_path, &state.work_dir(), &check_path).await {
        let _ = std::fs::remove_file(&check_path);
        return Err(e.to_string());
    }
    let _ = std::fs::remove_file(&check_path);

    // 3. 停止旧内核
    state
        .transition(CoreStatus::Stopping)
        .map_err(|e| e.to_string())?;
    if let Err(e) = do_stop(state).await {
        let _ = state.transition(CoreStatus::Error);
        return Err(e.to_string());
    }
    *state.started_at.write() = None;
    state
        .transition(CoreStatus::Stopped)
        .map_err(|e| e.to_string())?;

    // 4. 应用已校验的配置启动新内核；失败仍转 Error 允许重试
    state
        .transition(CoreStatus::Starting)
        .map_err(|e| e.to_string())?;
    if let Err(e) = apply_prepared(prepared, app, state).await {
        let _ = do_stop(state).await;
        let _ = state.transition(CoreStatus::Error);
        return Err(e);
    }
    *state.started_at.write() = Some(std::time::Instant::now());
    state
        .transition(CoreStatus::Running)
        .map_err(|e| e.to_string())?;

    // 与 start_core 一致：重启后自动测速选点
    if let Some(group) = state.main_selector.read().clone() {
        spawn_auto_select(
            app.clone(),
            state.api_addr.read().clone(),
            state.api_secret.read().clone(),
            group,
        );
    }
    Ok(())
}

/// 从 route.final 沿 selector 链解析「主选择组」：
/// 该组成员为叶子节点（或 urltest 组），用户的选择真实影响出口流量。
async fn resolve_main_selector(api: &KernelApi, final_group: &str) -> Option<String> {
    let proxies = api.get_proxies().await.ok()?;
    let map = proxies.get("proxies")?.as_object()?;

    let mut current = final_group.to_string();
    let mut visited = std::collections::HashSet::new();
    loop {
        // 防环（组互相引用）
        if !visited.insert(current.clone()) {
            return None;
        }
        let entry = map.get(&current)?;
        if entry.get("type")?.as_str()? != "Selector" {
            // final 直接指向 urltest/叶子：无可手选的 selector
            return None;
        }
        let now = entry.get("now")?.as_str()?.to_string();
        let now_entry = map.get(&now)?;
        if now_entry.get("type")?.as_str()? == "Selector" {
            current = now;
            continue;
        }
        return Some(current);
    }
}

/// 后台任务：对主选择组整组测速，自动切换到延迟最低的可用叶子节点
fn spawn_auto_select(app: AppHandle, api_addr: String, api_secret: String, group: String) {
    tauri::async_runtime::spawn(async move {
        use tauri::Emitter as _;
        let api = KernelApi::new(&api_addr, &api_secret);
        match auto_select_fastest(&api, &group).await {
            Ok(Some(best)) => {
                tracing::info!("auto selected fastest node in [{group}]: {best}");
                // 通知节点页刷新（当前选中项/延迟已变化）
                let _ = app.emit("proxies://changed", ());
            }
            Ok(None) => tracing::warn!("auto select: no alive leaf node in [{group}]"),
            Err(e) => tracing::warn!("auto select in [{group}] failed: {e}"),
        }
    });
}

/// 整组测速并选择最优叶子节点；返回被选中的节点名。
/// direct 延迟最低但无法翻墙，嵌套策略组也不可作为目标，均需排除。
async fn auto_select_fastest(api: &KernelApi, group: &str) -> anyhow::Result<Option<String>> {
    const GROUP_TYPES: [&str; 4] = ["Selector", "URLTest", "Fallback", "LoadBalance"];
    const BUILTIN_TAGS: [&str; 4] = ["direct", "block", "dns-out", "GLOBAL"];

    let delays = api.group_url_test(group, None, Some(3000)).await?;
    let proxies = api.get_proxies().await?;
    let empty = serde_json::Map::new();
    let map = proxies
        .get("proxies")
        .and_then(|p| p.as_object())
        .unwrap_or(&empty);

    let best = delays
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(name, _)| !BUILTIN_TAGS.contains(&name.as_str()))
        .filter(|(name, _)| {
            let kind = map
                .get(*name)
                .and_then(|e| e.get("type"))
                .and_then(|t| t.as_str())
                .unwrap_or_default();
            !GROUP_TYPES.contains(&kind)
        })
        .filter_map(|(name, delay)| delay.as_u64().map(|d| (name.clone(), d)))
        .min_by_key(|(_, d)| *d)
        .map(|(name, _)| name);

    if let Some(best) = &best {
        api.select_proxy(group, best).await?;
    }
    Ok(best)
}

/// 解析 sing-box 二进制位置，优先级：
/// 1. 环境变量 PRISM_KERNEL_PATH
/// 2. 内核工作目录内 sing-box[.exe]（首启自动下载落盘位置）
/// 3. 主程序同级目录 sing-box[.exe]
/// （iOS 内核内嵌于主程序，无独立二进制可解析）
#[cfg(not(target_os = "ios"))]
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
        main_selector: state.main_selector.read().clone(),
    })
}

#[tauri::command]
pub async fn set_mode(
    mode: RunMode,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let prev = *state.mode.read();
    if prev == mode {
        return Ok(());
    }
    *state.mode.write() = mode;

    // 持久化到 settings.json，重启应用后恢复同一模式（仪表盘显示才有权威来源）
    let mut settings = state.settings.read().clone();
    settings.mode = mode;
    state
        .save_settings(settings)
        .map_err(|e| format!("保存运行模式失败: {e}"))?;

    // 内核运行中：模式决定入站结构与系统代理行为，必须重建配置重启才能真实生效，
    // 否则显示的模式与实际代理效果必然不一致
    if *state.status.read() == CoreStatus::Running {
        tracing::info!("mode changed {prev:?} -> {mode:?}, restarting core");
        restart_core(&app, &state).await?;
    }
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
        .group_url_test(&req.group, req.url.as_deref(), req.timeout_ms)
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
        .map_err(|e| e.to_string())?;

    // 「切换策略时关闭连接」：断开后由应用按新节点重建，避免旧连接继续走旧节点
    if state.settings.read().close_connections_on_switch {
        let _ = state.kernel_api().close_all_connections().await;
    }
    Ok(())
}

/// 下载订阅并写入 profile.yaml。
/// 兼容两种返回：Clash YAML 明文、base64 包裹的内容。
#[tauri::command]
pub async fn update_subscription(
    url: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // 内核运行中时，直连失败的最终兜底是经本地 mixed 入站转发
    let local_proxy = (*state.status.read() == CoreStatus::Running)
        .then(|| state.settings.read().mixed_port);
    let mut text = fetch_remote_text(&url, local_proxy).await?;

    // base64 包裹：明文不含 YAML/JSON 结构时尝试解码
    if !text.contains("proxies") && !text.trim_start().starts_with('{') {
        if let Ok(decoded) = try_decode_base64(&text) {
            text = decoded;
        }
    }

    // URI 列表订阅（ss:// vmess:// vless:// trojan:// hysteria2:// tuic:// 分享链接）：
    // 解析为节点并合成 Clash YAML，复用统一 pipeline，不再拒绝
    if crate::core::uri_parser::is_uri_list(&text) {
        let (nodes, errors) = crate::core::uri_parser::parse_lines(&text);
        if nodes.is_empty() {
            return Err(format!(
                "URI 列表订阅解析失败：{}",
                errors.first().cloned().unwrap_or_else(|| "内容为空".into())
            ));
        }
        if !errors.is_empty() {
            tracing::warn!("subscription partial parse: {}", errors.join("；"));
        }
        // 节点名称去重（同名节点会互相覆盖）
        let mut seen = std::collections::HashSet::new();
        let mut proxies = Vec::new();
        for mut node in nodes {
            let base = node["name"].as_str().unwrap_or_default().to_string();
            let base = if base.is_empty() {
                format!(
                    "{}:{}",
                    node["server"].as_str().unwrap_or_default(),
                    node["port"]
                )
            } else {
                base
            };
            let mut name = base.clone();
            let mut i = 2;
            while !seen.insert(name.clone()) {
                name = format!("{base} #{i}");
                i += 1;
            }
            node["name"] = serde_json::json!(name);
            proxies.push(node);
        }
        let names: Vec<&str> = proxies
            .iter()
            .filter_map(|n| n["name"].as_str())
            .collect();
        let profile = serde_json::json!({
            "proxies": proxies,
            "proxy-groups": [
                { "name": "节点选择", "type": "select", "proxies": names }
            ],
            "rules": ["MATCH,节点选择"],
        });
        text = serde_yaml::to_string(&profile).map_err(|e| format!("订阅转换失败: {e}"))?;
    }

    if !text.contains("proxies") && !text.contains("{") {
        return Err("订阅内容无法识别：既不是 Clash YAML，也不是 sing-box JSON".into());
    }

    // 落盘前校验：防止机场返回 HTML 错误页/限流提示等空内容覆盖本地可用订阅
    if let Err(e) = crate::core::config_builder::validate_subscription_text(&text) {
        return Err(e);
    }

    // 每条订阅独立 profile 文件（多订阅模型），路径由 URL 哈希派生
    let profile_path = crate::core::store::profile_path(&state.data_dir, &url);
    if let Some(parent) = profile_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&profile_path, &text)
        .map_err(|e| format!("写入订阅文件失败: {}", e))?;
    tracing::info!("subscription saved to {}", profile_path.display());

    // 持久化订阅记录
    crate::core::store::upsert_subscription(&state.data_dir, &url)
        .map_err(|e| format!("保存订阅记录失败: {e}"))?;

    // 拉取订阅内声明的 http proxy-providers（失败只告警，不阻断订阅更新）
    refresh_providers(&state, &url, &text).await;

    // 内核运行中：重建配置重启，新订阅即刻生效
    if *state.status.read() == CoreStatus::Running {
        tracing::info!("subscription updated, restarting core");
        restart_core(&app, &state).await?;
    }

    Ok(())
}

/// 下载远程文本（订阅 / proxy-provider 共用）。
/// 伪装为 Clash 客户端，多数机场据此返回 Clash YAML；
/// 直连必须 no_proxy：内核运行时系统代理指向本进程，走系统代理会自回环。
///
/// 三级重试闭环：
/// 1. 直连（hyper 自动尝试 v6/v4）
/// 2. 强制 IPv4 源地址直连——部分网络 IPv6 出口为黑洞（AAAA 优先 + SYN 无
///    响应），0.0.0.0 绑定后 v6 目标因地址族不匹配立即跳过，秒级回退 v4
/// 3. 内核运行中 → 经本地 mixed 入站（127.0.0.1:mixed_port）转发——受限网络
///    直连被墙时走节点出境，实现「先启动内核再更新」的真实闭环
async fn fetch_remote_text(url: &str, local_proxy: Option<u16>) -> Result<String, String> {
    let mut last_err = String::new();

    // ---- 直连两连发（自动 + 强制 v4）----
    for force_v4 in [false, true] {
        let mut builder = reqwest::Client::builder()
            .user_agent("clash-verge/v2.0.0")
            .timeout(std::time::Duration::from_secs(30))
            // 连接阶段独立短超时：黑洞地址快速失败，尽快进入重试
            .connect_timeout(std::time::Duration::from_secs(10))
            .no_proxy();
        if force_v4 {
            builder = builder.local_address(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
        }
        match try_fetch_text(&builder.build().map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?, url)
            .await
        {
            Ok(text) => return Ok(text),
            Err(e) => {
                last_err = e;
                tracing::warn!("fetch remote text (force_v4={force_v4}) failed: {last_err}");
            }
        }
    }

    // ---- 内核运行中：经本地 mixed 入站转发 ----
    if let Some(port) = local_proxy {
        let proxy = reqwest::Proxy::all(&format!("http://127.0.0.1:{port}"))
            .map_err(|e| format!("配置本地代理失败: {e}"))?;
        let builder = reqwest::Client::builder()
            .user_agent("clash-verge/v2.0.0")
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .proxy(proxy);
        match try_fetch_text(&builder.build().map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?, url)
            .await
        {
            Ok(text) => {
                tracing::info!("fetch remote text succeeded via local mixed proxy :{port}");
                return Ok(text);
            }
            Err(e) => {
                last_err = e;
                tracing::warn!("fetch remote text via local proxy failed: {last_err}");
            }
        }
        Err(format!(
            "下载失败：{last_err}（直连与本地代理转发均已尝试，请检查节点可用性后重试）"
        ))
    } else {
        Err(format!(
            "下载失败：{last_err}（已尝试 IPv4 直连；若机器处于受限网络，请先启动内核再更新订阅）"
        ))
    }
}

/// 用给定客户端执行一次 GET 文本下载（状态码 / UTF-8 校验）
async fn try_fetch_text(client: &reqwest::Client, url: &str) -> Result<String, String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| describe_reqwest_error(&e))?;
    if !resp.status().is_success() {
        return Err(format!(
            "HTTP {}（订阅链接可能已失效）",
            resp.status()
        ));
    }
    let bytes = resp.bytes().await.map_err(|e| format!("读取响应失败: {e}"))?;
    String::from_utf8(bytes.to_vec()).map_err(|_| "内容不是有效 UTF-8 文本".to_string())
}

/// reqwest 顶层 Display 只有 "error sending request"，
/// 链式展开到最底层原因（DNS / 连接超时 / TLS 证书等）便于用户自救
fn describe_reqwest_error(e: &reqwest::Error) -> String {
    let mut msg = e.to_string();
    let mut src: Option<&dyn std::error::Error> = Some(&e);
    while let Some(s) = src.and_then(std::error::Error::source) {
        msg.push_str(&format!("（原因: {s}）"));
        src = Some(s);
    }
    msg
}

/// 解析订阅内的 proxy-providers（仅 http），逐个下载落盘
async fn refresh_providers(state: &AppState, sub_url: &str, profile_text: &str) {
    let view: crate::core::config_builder::ClashProfileView =
        match serde_yaml::from_str(profile_text) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("parse providers from subscription failed: {e}");
                return;
            }
        };

    for (name, provider) in view.proxy_providers {
        if provider.kind != "http" {
            continue;
        }
        let Some(url) = provider.url.filter(|u| !u.is_empty()) else {
            tracing::warn!("provider '{name}' missing url, skipped");
            continue;
        };
        let local_proxy = (*state.status.read() == CoreStatus::Running)
            .then(|| state.settings.read().mixed_port);
        match fetch_remote_text(&url, local_proxy).await {
            Ok(text) => {
                let path = crate::core::store::provider_path(&state.data_dir, sub_url, &name);
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(e) = std::fs::write(&path, &text) {
                    tracing::warn!("write provider '{name}' failed: {e}");
                } else {
                    tracing::info!("proxy-provider '{name}' refreshed");
                }
            }
            Err(e) => tracing::warn!("fetch proxy-provider '{name}' failed: {e}"),
        }
    }
}

/// 尝试 base64 解码（容错换行/空白），成功且结果为可读 UTF-8 时返回
fn try_decode_base64(input: &str) -> Result<String, String> {
    let compact: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    let decoded = STANDARD
        .decode(&compact)
        .map_err(|e| format!("not base64: {}", e))?;
    String::from_utf8(decoded).map_err(|e| format!("decoded bytes not UTF-8: {}", e))
}

/// 订阅记录（前端展示用）：在存储记录上附加节点数统计
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionDto {
    pub url: String,
    pub updated_at: i64,
    pub name: Option<String>,
    /// 是否参与配置合并
    pub enabled: bool,
    /// 节点数（解析该订阅自己的 profile 文件统计；文件缺失/解析失败为 None）
    pub node_count: Option<usize>,
}

/// 获取已保存的订阅记录（订阅页展示）
#[tauri::command]
pub async fn list_subscriptions(
    state: State<'_, AppState>,
) -> Result<Vec<SubscriptionDto>, String> {
    let records = crate::core::store::load_subscriptions(&state.data_dir);
    Ok(records
        .into_iter()
        .map(|r| {
            let path = crate::core::store::profile_path(&state.data_dir, &r.url);
            let node_count = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| count_profile_nodes(&text));
            SubscriptionDto {
                url: r.url,
                updated_at: r.updated_at,
                name: r.name,
                enabled: r.enabled,
                node_count,
            }
        })
        .collect())
}

/// 统计 profile 内容的 proxies 条目数；解析失败返回 None
fn count_profile_nodes(text: &str) -> Option<usize> {
    let value: serde_yaml::Value = serde_yaml::from_str(text).ok()?;
    value.get("proxies")?.as_sequence().map(|s| s.len())
}

/// 获取自定义规则（规则页编辑器；Clash 行格式，顺序即优先级）
#[tauri::command]
pub async fn get_custom_rules(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    Ok(crate::core::store::load_custom_rules(&state.data_dir))
}

/// 保存自定义规则（内核运行中自动重启生效）
#[tauri::command]
pub async fn save_custom_rules(
    rules: Vec<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    crate::core::store::save_custom_rules(&state.data_dir, &rules)
        .map_err(|e| format!("保存自定义规则失败: {e}"))?;
    if *state.status.read() == CoreStatus::Running {
        restart_core(&app, &state).await?;
    }
    Ok(())
}

/// 当前启用订阅数
fn enabled_subscription_count(state: &AppState) -> usize {
    crate::core::store::load_subscriptions(&state.data_dir)
        .iter()
        .filter(|r| r.enabled)
        .count()
}

/// 内核运行中至少需要一条启用订阅，避免走"重启时 profile 为空"的异常停机路径
fn ensure_not_last_enabled(state: &AppState) -> Result<(), String> {
    if *state.status.read() == CoreStatus::Running && enabled_subscription_count(state) <= 1 {
        return Err(
            "内核运行中至少需要保留一条启用订阅：请先停止内核，再执行该操作".into(),
        );
    }
    Ok(())
}

/// 启停单条订阅（多订阅模型；内核运行中自动重启生效）
#[tauri::command]
pub async fn toggle_subscription(
    url: String,
    enabled: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // 停用最后一条启用订阅前拦截（启用操作无需拦截）
    if !enabled {
        ensure_not_last_enabled(&state)?;
    }

    let found = crate::core::store::set_subscription_enabled(&state.data_dir, &url, enabled)
        .map_err(|e| format!("更新订阅状态失败: {e}"))?;
    if !found {
        return Err("订阅记录不存在".into());
    }
    if *state.status.read() == CoreStatus::Running {
        restart_core(&app, &state).await?;
    }
    Ok(())
}

/// 删除一条订阅记录（同时移除其 profile 文件；内核运行中自动重启生效）
#[tauri::command]
pub async fn delete_subscription(
    url: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    // 目标为启用状态时，删除前校验是否为最后一条
    let target_enabled = crate::core::store::load_subscriptions(&state.data_dir)
        .iter()
        .any(|r| r.url == url && r.enabled);
    if target_enabled {
        ensure_not_last_enabled(&state)?;
    }

    let removed = crate::core::store::delete_subscription(&state.data_dir, &url)
        .map_err(|e| format!("删除订阅失败: {e}"))?;
    if removed {
        std::fs::remove_file(crate::core::store::profile_path(&state.data_dir, &url)).ok();
        if *state.status.read() == CoreStatus::Running {
            restart_core(&app, &state).await?;
        }
    }
    Ok(removed)
}

/// 将授权状态写入内存并广播 pro://status（前端 store 据此刷新）
fn apply_entitlement(state: &State<'_, AppState>, next: EntitlementStatus) {
    *state.entitlement.write() = next.clone();
    let _ = state.app_handle.emit("pro://status", &next);
}

/// 激活码激活（邮箱+授权码；联网校验；服务端签名后本地缓存授权）
#[tauri::command]
pub async fn activate_pro(
    code: String,
    email: String,
    state: State<'_, AppState>,
) -> Result<EntitlementStatus, String> {
    let data_dir = state.data_dir.clone();
    let settings = state.settings.read().clone();
    let next = crate::core::license::activate(&data_dir, &settings, &code, &email)
        .await
        .map_err(|e| e.to_string())?;
    apply_entitlement(&state, next.clone());
    Ok(next)
}

/// 查询当前授权状态（读内存，不联网）
#[tauri::command]
pub async fn get_entitlement(
    state: State<'_, AppState>,
) -> Result<EntitlementStatus, String> {
    Ok(state.entitlement.read().clone())
}

/// 主动联网刷新授权（设置页「立即校验」按钮）
#[tauri::command]
pub async fn refresh_entitlement(
    state: State<'_, AppState>,
) -> Result<EntitlementStatus, String> {
    let data_dir = state.data_dir.clone();
    let settings = state.settings.read().clone();
    let next = crate::core::license::verify_remote(&data_dir, &settings)
        .await
        .map_err(|e| e.to_string())?;
    apply_entitlement(&state, next.clone());
    Ok(next)
}

/// 移除本机授权
#[tauri::command]
pub async fn deactivate_pro(
    state: State<'_, AppState>,
) -> Result<EntitlementStatus, String> {
    let next = crate::core::license::deactivate(&state.data_dir);
    apply_entitlement(&state, next.clone());
    Ok(next)
}

/// 移动端商店收据上报（apple | google），服务端校验后签发授权
#[tauri::command]
pub async fn submit_receipt(
    store: String,
    receipt: serde_json::Value,
    state: State<'_, AppState>,
) -> Result<EntitlementStatus, String> {
    let data_dir = state.data_dir.clone();
    let settings = state.settings.read().clone();
    let next = crate::core::license::submit_store_receipt(&data_dir, &settings, &store, receipt)
        .await
        .map_err(|e| e.to_string())?;
    apply_entitlement(&state, next.clone());
    Ok(next)
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
    #[cfg(target_os = "ios")]
    {
        let _ = app;
        *state.kernel_version.write() = Some(format!("{} （内置）", crate::core::kernel::IOS_EMBEDDED_VERSION));
        return Ok(kernel_info(&state));
    }
    #[cfg(not(target_os = "ios"))]
    {
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
}

fn kernel_info(state: &AppState) -> KernelInfoDto {
    #[cfg(target_os = "ios")]
    {
        let version = state
            .kernel_version
            .read()
            .clone()
            .or_else(|| Some(format!("{} （内置）", crate::core::kernel::IOS_EMBEDDED_VERSION)));
        KernelInfoDto {
            exists: true,
            path: None,
            version,
        }
    }
    #[cfg(not(target_os = "ios"))]
    {
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
}

#[tauri::command]
pub async fn get_connections(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    state
        .kernel_api()
        .get_connections()
        .await
        .map_err(|e| e.to_string())
}

/// 关闭单条连接（连接页行内按钮）
#[tauri::command]
pub async fn close_connection(
    id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .kernel_api()
        .close_connection(&id)
        .await
        .map_err(|e| e.to_string())
}

/// 关闭全部连接（连接页头部按钮）
#[tauri::command]
pub async fn close_all_connections(state: State<'_, AppState>) -> Result<(), String> {
    state
        .kernel_api()
        .close_all_connections()
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
    mut settings: UserSettings,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let prev = state.settings.read().clone();

    // 运行模式以 set_mode 为唯一入口（设置表单可能携带旧值，直接覆盖防止回退）
    settings.mode = *state.mode.read();

    // 自启动立即生效
    crate::platform::set_auto_start(settings.auto_start).map_err(|e| e.to_string())?;

    // 落盘 settings.json，重启后恢复
    let next = settings.clone();
    state
        .save_settings(settings)
        .map_err(|e| format!("保存设置失败: {e}"))?;

    // 影响入站/系统代理/出站结构的字段变更必须重建配置才能真实生效，
    // 否则系统代理指向旧端口、内核监听旧端口，显示与实际脱节
    let needs_restart = prev.mixed_port != next.mixed_port
        || prev.allow_lan != next.allow_lan
        || prev.system_proxy != next.system_proxy
        || prev.outbound_mode != next.outbound_mode
        || prev.ipv6 != next.ipv6
        || prev.block_quic != next.block_quic;
    if needs_restart && *state.status.read() == CoreStatus::Running {
        restart_core(&app, &state).await?;
    }
    Ok(())
}
