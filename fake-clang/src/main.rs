// 假 clang/ar：仅用于 `cargo check` 交叉检查时骗过 ring 等 crate 的 build script。
// clang 模式：
// - `-v`：输出 clang 版本特征串，使 cc crate 判定为 clang 家族
// - 编译调用：解析 `-o`，创建空输出文件后返回 0
// ar 模式（程序名含 "ar"）：
// - 形如 `ar crs libfoo.a obj.o ...`：在第二个参数位置创建空归档文件
// check 不链接，空目标文件不会影响最终产物检查。
use std::process::exit;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let prog = std::env::current_exe()
        .ok()
        .and_then(|p| {
            p.file_stem()
                .map(|s| s.to_string_lossy().to_lowercase())
        })
        .unwrap_or_default();

    let is_xcrun = prog.contains("xcrun");
    let is_ar = prog.contains("ar") && !prog.contains("clang") && !is_xcrun;

    if is_xcrun {
        // cc-rs 调用：
        //   xcrun --sdk iphoneos --show-sdk-path
        //   xcrun --sdk iphoneos --find clang
        let fake_sdk = r"D:\Prism\fake-sdk\iPhoneOS.sdk";
        let _ = std::fs::create_dir_all(fake_sdk);
        let args: Vec<String> = args;
        if args.iter().any(|a| a == "--show-sdk-path") {
            println!("{}", fake_sdk);
        } else if let Some(pos) = args.iter().position(|a| a == "--find") {
            // 对 --find <tool> 一律回假 clang 路径
            let _ = args.get(pos + 1);
            println!(r"D:\Prism\fake-bin\clang.exe");
        } else if args.iter().any(|a| a == "-v") {
            println!("xcrun version 17.0 (fake)");
        }
        exit(0);
    }

    if is_ar {
        // 参数通常为 ["crs"/"rcs"/... , "归档路径", ...对象文件]
        let target_path = args
            .iter()
            .skip(1)
            .find(|a| !a.starts_with('-') && a.ends_with(".a"))
            .cloned()
            .or_else(|| args.get(1).cloned());
        if let Some(archive) = target_path {
            // 写入合法的空归档魔数，rustc 读取 -L native 时不会报
            // "failed to add native library"
            let _ = std::fs::write(&archive, b"!<arch>\n");
        }
        exit(0);
    }

    if args.iter().any(|a| a == "-v") || args.iter().any(|a| a == "--version") {
        println!("clang version 17.0.0 (fake)");
        println!("Target: aarch64-linux-android");
        println!("Thread model: posix");
        exit(0);
    }

    // 找到 -o 后的输出路径并创建空文件
    if let Some(pos) = args.iter().position(|a| a == "-o") {
        if let Some(out) = args.get(pos + 1) {
            let _ = std::fs::File::create(out);
        }
    }

    // 预处理/探测类调用也一律成功
    exit(0);
}
