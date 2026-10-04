//! macOS 系统集成
//!
//! 通过 networksetup 对所有活跃网络服务设置/清除代理；
//! 自启动通过 ~/Library/LaunchAgents 下的 LaunchAgent plist 实现。

use std::io;
use std::process::Command;

/// 获取所有活跃网络服务名称（如 Wi-Fi、Ethernet）
fn network_services() -> Vec<String> {
    let Ok(output) = Command::new("networksetup")
        .arg("-listnetworkserviceorder")
        .output()
    else {
        return vec![];
    };

    let text = String::from_utf8_lossy(&output.stdout);

    text.lines()
        .filter_map(|line| {
            // 形如：(1) Wi-Fi
            let bytes = line.trim_start().as_bytes();
            if bytes.first() != Some(&b'(') {
                return None;
            }
            let close = line.find(')')?;
            // ") " 之后到行尾
            let name = line[close + 1..].trim();
            // 过滤硬件端口说明行（它们以 '(' 开头但不以序号开头的情况已被排除）
            if name.is_empty() || name.starts_with('(') {
                return None;
            }
            Some(name.to_string())
        })
        .collect()
}

fn run_networksetup(args: &[&str]) -> io::Result<()> {
    let status = Command::new("networksetup").args(args).status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "networksetup {:?} failed: {}",
            args, status
        )));
    }
    Ok(())
}

pub fn set_system_proxy(host: &str, port: u16) -> io::Result<()> {
    let port = port.to_string();

    for service in network_services() {
        run_networksetup(&["-setwebproxy", &service, host, &port])?;
        run_networksetup(&["-setsecurewebproxy", &service, host, &port])?;
        run_networksetup(&["-setsocksfirewallproxy", &service, host, &port])?;
    }
    Ok(())
}

pub fn clear_system_proxy() -> io::Result<()> {
    for service in network_services() {
        run_networksetup(&["-setwebproxystate", &service, "off"])?;
        run_networksetup(&["-setsecurewebproxystate", &service, "off"])?;
        run_networksetup(&["-setsocksfirewallproxystate", &service, "off"])?;
    }
    Ok(())
}

/// 开机自启动：LaunchAgent plist
pub fn set_auto_start(enabled: bool) -> io::Result<()> {
    let home = std::env::var("HOME").map_err(|_| io::Error::other("no HOME"))?;
    let dir = format!("{}/Library/LaunchAgents", home);
    let plist_path = format!("{}/com.prism.proxy.plist", dir);

    if enabled {
        std::fs::create_dir_all(&dir)?;
        let exe = std::env::current_exe()?;
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>com.prism.proxy</string>
    <key>ProgramArguments</key>
    <array><string>{}</string></array>
    <key>RunAtLoad</key><true/>
</dict>
</plist>
"#,
            exe.display()
        );
        std::fs::write(&plist_path, plist)?;
    } else if std::path::Path::new(&plist_path).exists() {
        std::fs::remove_file(&plist_path)?;
    }

    Ok(())
}
