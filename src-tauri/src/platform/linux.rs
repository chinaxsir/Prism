//! Linux 系统集成
//!
//! 通过 GNOME gsettings 设置系统代理（覆盖 GNOME 及兼容实现，如 Budgie/Cinnamon）；
//! 自启动通过 ~/.config/autostart 下的 .desktop 文件实现。
//!
//! 无图形/无 gsettings 的环境（headless、纯 TUN 服务器）中静默跳过。

use std::io;
use std::process::Command;

fn gsettings(args: &[&str]) -> bool {
    Command::new("gsettings")
        .args(args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn set_system_proxy(host: &str, port: u16) -> io::Result<()> {
    let port = port.to_string();

    // gsettings 不可用时静默返回（例如 CI/headless）
    if !gsettings(&["get", "org.gnome.system.proxy", "mode"]) {
        tracing::debug!("gsettings unavailable, skip setting system proxy");
        return Ok(());
    }

    gsettings(&["set", "org.gnome.system.proxy", "mode", "'manual'"]);

    for scheme in ["http", "https", "socks"] {
        gsettings(&[
            "set",
            &format!("org.gnome.system.proxy.{}", scheme),
            "host",
            &format!("'{}'", host),
        ]);
        gsettings(&[
            "set",
            &format!("org.gnome.system.proxy.{}", scheme),
            "port",
            &port,
        ]);
    }

    Ok(())
}

pub fn clear_system_proxy() -> io::Result<()> {
    gsettings(&["set", "org.gnome.system.proxy", "mode", "'none'"]);
    Ok(())
}

/// 开机自启动：XDG autostart .desktop 文件
pub fn set_auto_start(enabled: bool) -> io::Result<()> {
    let home = std::env::var("HOME").map_err(|_| io::Error::other("no HOME"))?;
    let dir = format!("{}/.config/autostart", home);
    let desktop_path = format!("{}/prism-proxy.desktop", dir);

    if enabled {
        std::fs::create_dir_all(&dir)?;
        let exe = std::env::current_exe()?;
        let desktop = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Prism Proxy\n\
             Exec=\"{}\"\n\
             Terminal=false\n\
             X-GNOME-Autostart-enabled=true\n",
            exe.display()
        );
        std::fs::write(&desktop_path, desktop)?;
    } else if std::path::Path::new(&desktop_path).exists() {
        std::fs::remove_file(&desktop_path)?;
    }

    Ok(())
}
