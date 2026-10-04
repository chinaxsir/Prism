// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    install_panic_hook();
    prism_proxy_lib::run()
}

/// panic 兜底：先输出默认 panic 信息，再尽力清理系统代理并强杀内核，
/// 避免崩溃后系统代理残留导致断网、sing-box 进程孤儿化。
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);

        if let Err(e) = prism_proxy_lib::platform::clear_system_proxy() {
            eprintln!("panic hook: clear system proxy failed: {e}");
        }
        if let Some(pid) = prism_proxy_lib::core::kernel::current_kernel_pid() {
            prism_proxy_lib::platform::kill_pid(pid);
        }
    }));
}
