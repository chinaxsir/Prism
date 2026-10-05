pub mod core;
pub mod ipc;
pub mod platform;
pub mod plugins;

#[cfg(desktop)]
use tauri::menu::{Menu, MenuItem};
#[cfg(desktop)]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};
use tracing_subscriber::EnvFilter;

/// 显示并聚焦主窗口（托盘左键 / 菜单“显示主窗口”）
#[cfg(desktop)]
fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// 真正退出：先回收内核/TUN/系统代理，再退出进程
fn quit_app(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(state) = app.try_state::<core::state::AppState>() {
            if let Err(e) = ipc::commands::do_stop(&state).await {
                tracing::error!("exit cleanup failed: {e}");
            }
        }
        app.exit(0);
    });
}

pub fn run() {
    // RUST_LOG 未设置时默认 info（静音高频网络库日志）
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,want=off,mio=off,polling=off"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_process::init());
    // 移动端 IAP（StoreKit2 / Google Play Billing），桌面不编译
    #[cfg(any(target_os = "android", target_os = "ios"))]
    let builder = builder.plugin(tauri_plugin_iap::init());

    let app = builder
        // 注意：日志统一走 tracing-subscriber（见 run() 开头），
        // 它已注册全局 log facade，再注册 tauri-plugin-log 会导致
        // “attempted to set a logger after the logging system was already initialized” panic
        .setup(|app| {
            // 数据目录：settings.json / subscriptions.json / kernel/（config.json 等）
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir).ok();
            app.manage(core::state::AppState::new(data_dir.clone(), app.handle().clone()));

            // 启动后台联网校验授权：结果写回内存并广播；失败（离线）不阻塞启动
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let state = handle.state::<core::state::AppState>();
                let settings = state.settings.read().clone();
                match core::license::verify_remote(&data_dir, &settings).await {
                    Ok(next) => {
                        tracing::info!("license verified: {}", next.status);
                        *state.entitlement.write() = next.clone();
                        let _ = handle.emit("pro://status", &next);
                    }
                    Err(e) => tracing::warn!("license verify failed: {e}"),
                }
            });

            // 系统托盘（仅桌面端；menu/tray API 在移动端被 cfg 移除）：
            // 左键单击显示主窗口；菜单提供 显示/退出
            #[cfg(desktop)]
            {
                let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
                let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
                let menu = Menu::with_items(app, &[&show, &quit])?;
                let mut tray = TrayIconBuilder::with_id("main-tray")
                    .menu(&menu)
                    .tooltip("Prism Proxy")
                    .show_menu_on_left_click(false)
                    .on_menu_event(|app, event| match event.id().as_ref() {
                        "show" => show_main_window(app),
                        "quit" => quit_app(app),
                        _ => {}
                    })
                    .on_tray_icon_event(|tray, event| {
                        if let TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        } = event
                        {
                            show_main_window(tray.app_handle());
                        }
                    });
                if let Some(icon) = app.default_window_icon() {
                    tray = tray.icon(icon.clone());
                }
                tray.build(app)?;
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // 点关闭按钮 → 隐藏到托盘常驻；真正退出走托盘菜单“退出”。
            // hide() 仅桌面端可用；移动端无关闭窗口语义
            #[cfg(desktop)]
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
            let _ = event;
        })
        .invoke_handler(tauri::generate_handler![
            ipc::commands::start_core,
            ipc::commands::stop_core,
            ipc::commands::get_core_status,
            ipc::commands::set_mode,
            ipc::commands::url_test,
            ipc::commands::select_proxy,
            ipc::commands::update_subscription,
            ipc::commands::list_subscriptions,
            ipc::commands::toggle_subscription,
            ipc::commands::delete_subscription,
            ipc::commands::get_custom_rules,
            ipc::commands::save_custom_rules,
            ipc::commands::activate_pro,
            ipc::commands::get_entitlement,
            ipc::commands::refresh_entitlement,
            ipc::commands::deactivate_pro,
            ipc::commands::submit_receipt,
            ipc::commands::get_connections,
            ipc::commands::close_connection,
            ipc::commands::close_all_connections,
            ipc::commands::get_traffic_stats,
            ipc::commands::get_rules,
            ipc::commands::get_proxy_groups,
            ipc::commands::get_settings,
            ipc::commands::save_settings,
            ipc::commands::get_kernel_info,
            ipc::commands::ensure_kernel,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        // 退出兜底：防止系统代理残留 / sing-box 孤儿进程
        if let RunEvent::ExitRequested { api, .. } = event {
            api.prevent_exit();
            quit_app(app_handle);
        }
    });
}
