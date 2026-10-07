//! 构建脚本：
//! 1. tauri_build：能力/codegen，并在 Android 编译期生成
//!    gen/android/tauri.settings.gradle（缺失会导致 Gradle settings 评估失败）、
//!    注入 desktop/mobile cfg 别名（缺它会报 unexpected `cfg` condition `desktop`）。
//! 2. iOS / Android 目标额外链接内嵌 sing-box 静态库（ios-kernel/libprismkernel*.a，
//!    由 CI 用 Go c-archive 构建，见 .github/workflows/mobile.yml）——
//!    两平台均进程内运行内核，不携带任何需要联网下载的文件。

use std::path::{Path, PathBuf};

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let kernel_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ios-kernel");

    match target_os.as_str() {
        "ios" => {
            println!("cargo:rustc-link-search=native={}", kernel_dir.display());
            println!("cargo:rustc-link-lib=static=prismkernel");
            // Go runtime / x509 根证书在 Apple 平台的依赖框架
            for fw in ["Security", "Foundation", "CoreFoundation", "SystemConfiguration"] {
                println!("cargo:rustc-link-lib=framework={fw}");
            }
            // Go c-archive iOS 运行时还依赖 libresolv（res_9_ninit/nclose/nsearch）与 libz
            for lib in ["resolv", "z"] {
                println!("cargo:rustc-link-lib={lib}");
            }
            println!(
                "cargo:rerun-if-changed={}",
                kernel_dir.join("libprismkernel.a").display()
            );
        }
        "android" => {
            // c-archive 按架构单独命名（CI 在 ios-kernel/ 一次产出四个）
            let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
            let name = match arch.as_str() {
                "aarch64" => "prismkernel-arm64",
                "arm" => "prismkernel-arm",
                "x86" => "prismkernel-386",
                "x86_64" => "prismkernel-amd64",
                other => panic!("android: 内嵌内核不支持的架构: {other}"),
            };
            println!("cargo:rustc-link-search=native={}", kernel_dir.display());
            println!("cargo:rustc-link-lib=static={name}");
            // Go android 运行时需要 liblog（__android_log_print）
            println!("cargo:rustc-link-lib=log");
            println!(
                "cargo:rerun-if-changed={}",
                Path::new(&kernel_dir)
                    .join(format!("lib{name}.a"))
                    .display()
            );
        }
        _ => {}
    }

    tauri_build::build()
}
