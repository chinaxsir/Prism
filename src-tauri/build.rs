//! 构建脚本：iOS 目标链接内嵌 sing-box 静态库（ios-kernel/libprismkernel.a，
//! 由 CI 用 Go c-archive 构建，见 .github/workflows/mobile.yml）。
//! 其他平台无任何影响。

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
}
