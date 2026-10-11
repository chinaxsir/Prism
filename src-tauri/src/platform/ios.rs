//! iOS 隧道控制桥接：
//!
//! 两条启动链路：
//! 1. VPN 模式（RunMode::Tun）：主 App 的 Rust 通过 C ABI 调用主 App 内
//!    Swift 暴露的控制函数（PrismTunnelController.swift，@_cdecl），由
//!    NETunnelProviderManager 启动/停止独立的 NEPacketTunnelProvider 扩展。
//!    内核（Go libbox）与 TUN 全部运行在扩展进程内。
//! 2. 本地代理模式（RunMode::SystemProxy，默认）：跳过 NE，Rust 直接 FFI
//!    调用主 App 内静态链接的 Go 内核导出（PrismKernelStart，ios-kernel/
//!    main.go c-archive），sing-box 在主 App 进程内以 mixed 入站监听
//!    127.0.0.1:mixed_port。无任何特殊 entitlement，免费 Apple ID
//!    （SideStore/AltStore 签名）即可安装使用。

use std::ffi::{CStr, CString};
use std::path::Path;

use anyhow::{Context, Result, anyhow};

unsafe extern "C" {
    /// 返回 0 = 成功；负数 = 错误（具体错误在扩展/系统日志中）
    fn prism_ios_vpn_start(config_content: *const std::os::raw::c_char) -> i32;
    fn prism_ios_vpn_stop();
}

/// Go 内核 c-archive 导出（ios-kernel/main.go；符号随 libprismkernel.a
/// 静态链入主 App，见 build.rs）。返回 NULL = 成功，非 NULL = 错误串
/// （调用方负责 PrismKernelFree 释放）。
unsafe extern "C" {
    fn PrismKernelStart(
        config_path: *const std::os::raw::c_char,
        work_dir: *const std::os::raw::c_char,
    ) -> *mut std::os::raw::c_char;
    fn PrismKernelStop();
    fn PrismKernelFree(p: *mut std::os::raw::c_char);
}

pub fn start_tunnel(config_content: &str) -> Result<()> {
    let cc = CString::new(config_content).context("invalid config string")?;
    let rc = unsafe { prism_ios_vpn_start(cc.as_ptr()) };
    if rc == 0 {
        Ok(())
    } else {
        // Swift 侧把 NSError 完整描述写入 NSTemporaryDirectory()/
        // prism_vpn_start_error.txt（iOS 沙盒 tmp，与 std::env::temp_dir 一致）
        let detail = std::fs::read_to_string(
            std::env::temp_dir().join("prism_vpn_start_error.txt")
        )
        .unwrap_or_default();
        if detail.is_empty() {
            Err(anyhow!(
                "NETunnelProviderManager 启动失败（代码 {rc}）：请检查系统 VPN 授权与扩展安装"
            ))
        } else {
            Err(anyhow!(
                "VPN 启动失败（代码 {rc}）：{detail}"
            ))
        }
    }
}

pub fn stop_tunnel() {
    unsafe { prism_ios_vpn_stop() }
}

/// 本地代理模式：主 App 进程内直接启动 sing-box（mixed 入站 127.0.0.1）。
/// config.json 必须已写入 config_path（KernelHandle::spawn 负责）。
pub fn start_local_proxy(config_path: &Path, work_dir: &Path) -> Result<()> {
    let cp = CString::new(config_path.to_string_lossy().as_bytes())
        .context("invalid config path")?;
    let wd = CString::new(work_dir.to_string_lossy().as_bytes())
        .context("invalid work dir")?;
    let err = unsafe { PrismKernelStart(cp.as_ptr(), wd.as_ptr()) };
    if err.is_null() {
        Ok(())
    } else {
        let msg = unsafe { CStr::from_ptr(err) }.to_string_lossy().into_owned();
        unsafe { PrismKernelFree(err) };
        Err(anyhow!("本地代理内核启动失败: {msg}"))
    }
}

pub fn stop_local_proxy() {
    unsafe { PrismKernelStop() }
}
