//! 构建脚本：
//! 1. tauri_build：能力/codegen，并在 Android 编译期生成
//!    gen/android/tauri.settings.gradle（缺失会导致 Gradle settings 评估失败）、
//!    注入 desktop/mobile cfg 别名（缺它会报 unexpected `cfg` condition `desktop`）。
//! 2. iOS 目标额外链接内嵌 sing-box 静态库（ios-kernel/libprismkernel.a，
//!    由 CI 用 Go c-archive 构建，见 .github/workflows/mobile.yml）。

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios") {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ios-kernel");
        println!("cargo:rustc-link-search=native={}", dir.display());
        println!("cargo:rustc-link-lib=static=prismkernel");
        // Go runtime / x509 根证书在 Apple 平台的依赖框架
        for fw in ["Security", "Foundation", "CoreFoundation", "SystemConfiguration"] {
            println!("cargo:rustc-link-lib=framework={fw}");
        }
        println!(
            "cargo:rerun-if-changed={}",
            dir.join("libprismkernel.a").display()
        );
    }

    tauri_build::build()
}
