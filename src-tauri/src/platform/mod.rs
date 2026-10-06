//! 平台系统集成层
//!
//! 职责：
//! - 系统代理设置/清除（Windows 注册表 / macOS networksetup / Linux gsettings）
//! - 管理员/root 权限检测（TUN 模式需要）
//! - 开机自启动注册

#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "linux")]
pub mod linux;

/// 设置系统代理（HTTP/HTTPS/SOCKS 统一指向 mixed 端口）
pub fn set_system_proxy(host: &str, port: u16) -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    windows::set_system_proxy(host, port)?;
    #[cfg(target_os = "macos")]
    macos::set_system_proxy(host, port)?;
    #[cfg(target_os = "linux")]
    linux::set_system_proxy(host, port)?;

    tracing::info!("system proxy set -> {}:{}", host, port);
    Ok(())
}

/// 清除系统代理（必须在停止/崩溃时兜底调用，防止代理残留断网）
pub fn clear_system_proxy() -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    windows::clear_system_proxy()?;
    #[cfg(target_os = "macos")]
    macos::clear_system_proxy()?;
    #[cfg(target_os = "linux")]
    linux::clear_system_proxy()?;

    tracing::info!("system proxy cleared");
    Ok(())
}

/// 强制结束指定 pid 的内核进程（panic 兜底使用，尽力而为不返回错误）
#[cfg_attr(any(target_os = "android", target_os = "ios"), allow(unused_variables))]
pub fn kill_pid(pid: u32) {
    #[cfg(target_os = "windows")]
    {
        windows::kill_pid(pid);
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    unsafe {
        let _ = libc::kill(pid as i32, libc::SIGKILL);
    }
}

/// 是否具有 TUN 所需的提权（Windows 管理员 / Unix root）
pub fn has_elevated_privilege() -> bool {
    #[cfg(target_os = "windows")]
    {
        windows::is_elevated()
    }
    #[cfg(not(target_os = "windows"))]
    {
        unsafe { libc::geteuid() == 0 }
    }
}

/// 设置/取消开机自启动
pub fn set_auto_start(enabled: bool) -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    windows::set_auto_start(enabled)?;
    #[cfg(target_os = "macos")]
    macos::set_auto_start(enabled)?;
    #[cfg(target_os = "linux")]
    linux::set_auto_start(enabled)?;

    tracing::info!("auto start -> {}", enabled);
    Ok(())
}
