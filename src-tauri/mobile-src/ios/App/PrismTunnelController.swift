// Prism 主 App 侧隧道控制：Rust 经 C ABI（@_cdecl）调用，
// 通过 NETunnelProviderManager 启动/停止 PrismVPN 网络扩展。
// 内核（Go libbox）与 TUN 全部运行在扩展进程内。

import Foundation
import NetworkExtension

/// 扩展 Bundle ID：优先取 App 包 PlugIns 内【实际存在】的
/// packet-tunnel-provider appex 的真实 CFBundleIdentifier（防构建侧
/// ID 漂移导致 providerBundleIdentifier 与实际 appex 不一致）；
/// 取不到时回落 主 App Bundle ID + ".PrismVPN"
private let extensionBundleIdentifier: String = {
    let main = Bundle.main.bundleIdentifier ?? "com.prism.proxy"
    let derived = "\(main).PrismVPN"
    guard let actual = PrismTunnelController.discoverExtensionBundleID() else {
        NSLog("[PrismVPN] PlugIns 内未找到 appex, 回落推导 ID=\(derived)")
        return derived
    }
    if actual != derived {
        NSLog("[PrismVPN] appex 实际 ID=\(actual) 与推导 ID=\(derived) 不同, 采用实际 ID")
    }
    return actual
}()

/// App Group 共享容器 ID（与扩展端一致）
/// TrollStore 可签任意 entitlement，App Group 不需要 Apple 注册
private let prismAppGroupID = "group.com.prism.proxy"

private let managerServerTag = "Prism"

// MARK: - C ABI 入口（供 Rust 调用）

@_cdecl("prism_ios_vpn_start")
func prismIosVpnStart(_ config: UnsafePointer<CChar>) -> Int32 {
    let configText = String(cString: config)

    // 写到 App Group 共享容器，避开 NETunnelProviderManager
    // providerConfiguration 512KB 限制（用户配置 2.47MB）
    let configPath: String
    if let groupURL = FileManager.default.containerURL(
        forSecurityApplicationGroupIdentifier: prismAppGroupID
    ) {
        configPath = groupURL.appendingPathComponent("prism_config.json").path
        NSLog("[PrismVPN] App Group 可用: \(configPath)")
    } else {
        // 回落到主 App tmp（与扩展进程不共享，仅小配置场景可用）
        configPath = (NSTemporaryDirectory() as NSString)
            .appendingPathComponent("prism_config.json")
        NSLog("[PrismVPN] App Group 不可用, 回落 tmp: \(configPath) (扩展可能读不到)")
    }
    do {
        try configText.write(toFile: configPath, atomically: true, encoding: .utf8)
    } catch {
        NSLog("[PrismVPN] 写共享配置失败: \(error.localizedDescription)")
        let path = (NSTemporaryDirectory() as NSString)
            .appendingPathComponent("prism_vpn_start_error.txt")
        try? "写共享配置失败: \(error.localizedDescription)"
            .write(toFile: path, atomically: true, encoding: .utf8)
        return -1
    }

    let semaphore = DispatchSemaphore(value: 0)
    var result: Int32 = 0
    var errorDesc: String = ""

    PrismTunnelController.shared.start(configPath: configPath) { error in
        if let error = error {
            result = -1
            errorDesc = PrismTunnelController.describe(error: error)
        }
        semaphore.signal()
    }
    semaphore.wait()

    if result != 0 {
        // 把详细错误写入沙盒 tmp（与 Rust std::env::temp_dir() 同一路径，
        // iOS 沙盒内 /tmp 并不存在，必须用 NSTemporaryDirectory()）
        let buildId = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "?"
        let groupOk = FileManager.default.containerURL(
            forSecurityApplicationGroupIdentifier: "group.com.prism.proxy") != nil
        let diag = PrismTunnelController.diagnosePackaging()
        let sig = PrismTunnelController.diagnoseAppexEntitlements()
        let preflight = PrismTunnelController.diagnoseAppexPreflight()
        // 检查 appex bundle seal 是否存在
        var sealOk = "未检查"
        if let pluginsURL = Bundle.main.builtInPlugInsURL,
           let entries = try? FileManager.default.contentsOfDirectory(atPath: pluginsURL.path),
           let appexName = entries.first(where: { $0.hasSuffix(".appex") })
        {
            let sealPath = pluginsURL.appendingPathComponent(appexName)
                .appendingPathComponent("_CodeSignature/CodeResources").path
            sealOk = FileManager.default.fileExists(atPath: sealPath) ? "有" : "无"
        }
        let staticCheck = PrismTunnelController.diagnoseAppexStaticCode()
        let full = "\(errorDesc)\n[诊断] build=\(buildId) app=\(Bundle.main.bundleIdentifier ?? "?") ext=\(extensionBundleIdentifier) group=\(groupOk ? "OK" : "nil") managers=\(PrismTunnelController.lastManagerCount) seal=\(sealOk) static=\(staticCheck) lsReg=\(PrismTunnelController.lsRegisterResult) lsAppWS=\(PrismTunnelController.lsAppWorkspaceResult) pluginkit=\(PrismTunnelController.pluginkitResult) PlugIns: \(diag) ; appex签名: \(sig) ; appex预检: \(preflight)\n[提示] Code 14 = 系统未注册扩展。请依次尝试：1) 设置→隐私与安全性→开发者模式→打开→重启手机（iOS 16+ NetworkExtension 必须） 2) TrollStore→设置→刷新App注册 3) 彻底卸载→重启→重装"
        let path = (NSTemporaryDirectory() as NSString)
            .appendingPathComponent("prism_vpn_start_error.txt")
        NSLog("[PrismVPN] 写错误详情到 \(path): \(full)")
        try? full.write(
            toFile: path,
            atomically: true,
            encoding: .utf8
        )
    }
    return result
}

@_cdecl("prism_ios_vpn_stop")
func prismIosVpnStop() {
    let semaphore = DispatchSemaphore(value: 0)
    PrismTunnelController.shared.stop { _ in
        semaphore.signal()
    }
    semaphore.wait()
}

// MARK: - 控制器

final class PrismTunnelController {
    static let shared = PrismTunnelController()

    private init() {}

    /// 最近一次 loadAllFromPreferences 可见的配置数。诊断用：
    /// 若系统设置里存在旧 Prism 条目而此处为 0，说明该条目属于
    /// 旧签名身份，App 内 API 不可见/不可删，必须去系统设置手动删除
    static var lastManagerCount = -1

    /// 把 NSError 完整描述（domain/code/userInfo）输出，便于真机定位
    static func describe(error: Error) -> String {
        let ns = error as NSError
        let userInfo = ns.userInfo.map { (k, v) -> String in
            "\(k)=\(v)"
        }.joined(separator: "; ")
        return "domain=\(ns.domain) code=\(ns.code) desc=\(ns.localizedDescription) userInfo={\(userInfo)}"
    }

    /// 遍历 App 包 PlugIns 下的 .appex，返回声明了 packet-tunnel-provider
    /// 扩展点的那个的真实 CFBundleIdentifier；找不到返回 nil
    static func discoverExtensionBundleID() -> String? {
        guard let pluginsURL = Bundle.main.builtInPlugInsURL,
              let entries = try? FileManager.default.contentsOfDirectory(
                atPath: pluginsURL.path)
        else { return nil }
        for entry in entries.sorted() where entry.hasSuffix(".appex") {
            guard let bundle = Bundle(url: pluginsURL.appendingPathComponent(entry)),
                  let info = bundle.infoDictionary
            else { continue }
            let point = (info["NSExtension"] as? [String: Any])?["NSExtensionPointIdentifier"] as? String
            if point == "com.apple.networkextension.packet-tunnel-provider",
               let bid = info["CFBundleIdentifier"] as? String, !bid.isEmpty {
                return bid
            }
        }
        return nil
    }

    /// 对 appex 做不加载、不启动扩展的完整性预检。
    /// 两条系统级路径在 iOS 上都不可用：
    /// - Bundle 没有 preflightCheck 方法（编译错误）
    /// - SecStaticCode/SecCode 系列是 macOS-only SPI，iOS 上即便
    ///   import Security 也「cannot find type SecStaticCode in scope」
    /// 因此这里只做文件级预检：bundle 可定位、Info.plist 扩展点正确、
    /// 可执行文件存在且 Mach-O 魔数合法。签名与 entitlements 的深度
    /// 诊断（手动解析 CS Superblob）见 diagnoseAppexEntitlements。
    static func diagnoseAppexPreflight() -> String {
        guard let pluginsURL = Bundle.main.builtInPlugInsURL,
              let entries = try? FileManager.default.contentsOfDirectory(
                atPath: pluginsURL.path),
              let appexName = entries.first(where: { $0.hasSuffix(".appex") }),
              let bundle = Bundle(url: pluginsURL.appendingPathComponent(appexName))
        else { return "appex bundle 不可定位" }

        guard let info = bundle.infoDictionary,
              let ext = info["NSExtension"] as? [String: Any],
              let point = ext["NSExtensionPointIdentifier"] as? String,
              point == "com.apple.networkextension.packet-tunnel-provider"
        else { return "Info.plist 扩展点声明缺失或错误" }

        guard let execURL = bundle.executableURL,
              let data = try? Data(contentsOf: execURL),
              data.count >= 4
        else { return "可执行文件缺失或不可读" }

        // Mach-O 魔数（arm64 thin 为小端 0xFEEDFACF；同时兼容大端变体
        // 与 FAT/通用二进制 0xCAFEBABE）
        let magic = data.prefix(4).withUnsafeBytes { $0.load(as: UInt32.self) }
        let valid: Set<UInt32> = [
            0xfeedfacf, 0xcffaedfe, // MH_MAGIC_64
            0xfeedface, 0xcefaedfe, // MH_MAGIC
            0xbebafeca, 0xcafebabe, // FAT 32
            0xbfbafeca, 0xcafebabf, // FAT 64
        ]
        guard valid.contains(magic) else {
            return String(format: "可执行文件 Mach-O 魔数非法: 0x%08x", magic)
        }
        return "ok"
    }

    /// 解析 appex 可执行文件内嵌代码签名（CS Superblob），报告
    /// legacy entitlements（槽位5）与 DER entitlements（槽位7）是否存在
    /// 及关键键值。用于确认 TrollStore 重签后扩展权限是否真实存活：
    /// nesessionmanager 解析 provider 时校验这些，缺失即报 code=14。
    ///
    /// 字节序注意（曾因此误报）：Mach-O 头与 Load Command 是小端序；
    /// 代码签名 Superblob 内部是大端（网络）序；FAT 头是大端序。
    static func diagnoseAppexEntitlements() -> String {
        guard let pluginsURL = Bundle.main.builtInPlugInsURL,
              let entries = try? FileManager.default.contentsOfDirectory(
                atPath: pluginsURL.path),
              let appexName = entries.first(where: { $0.hasSuffix(".appex") }),
              let bundle = Bundle(url: pluginsURL.appendingPathComponent(appexName)),
              let execURL = bundle.executableURL,
              let data = try? Data(contentsOf: execURL)
        else { return "无法读取 appex 可执行文件" }

        func u8(_ off: Int) -> Int {
            guard off >= 0, off < data.count else { return 0 }
            return Int(data[data.index(data.startIndex, offsetBy: off)])
        }
        func le32(_ off: Int) -> Int {
            u8(off) | (u8(off + 1) << 8) | (u8(off + 2) << 16) | (u8(off + 3) << 24)
        }
        func be32(_ off: Int) -> Int {
            (u8(off) << 24) | (u8(off + 1) << 16) | (u8(off + 2) << 8) | u8(off + 3)
        }

        // 定位 Mach-O slice：FAT(大端 ca fe ba be/bf) 或 thin(小端 cf fa ed fe)
        var base = 0
        if u8(0) == 0xcf, u8(1) == 0xfa, u8(2) == 0xed, u8(3) == 0xfe {
            base = 0  // MH_MAGIC_64 thin
        } else if u8(0) == 0xca, u8(1) == 0xfe,
                  (u8(2) == 0xba && (u8(3) == 0xbe || u8(3) == 0xbf)) {
            // FAT_MAGIC / FAT_MAGIC_64：找 arm64(0x0100000c) slice
            let nfat = be32(4)
            var found = -1
            for i in 0..<min(nfat, 16) {
                let aoff = 8 + i * 20
                if be32(aoff) == 0x0100000c { found = be32(aoff + 8) }  // arch.offset
            }
            guard found > 0 else { return "FAT 内无 arm64 slice" }
            base = found
        } else {
            return "非 arm64 Mach-O (magic=\(String(format: "%02x%02x%02x%02x", u8(0), u8(1), u8(2), u8(3)))"
        }

        guard u8(base) == 0xcf, u8(base + 1) == 0xfa,
              u8(base + 2) == 0xed, u8(base + 3) == 0xfe else {
            return "slice magic 异常"
        }
        let ncmds = le32(base + 16)
        var off = base + 32
        var sigFileOff = -1
        for _ in 0..<min(ncmds, 256) {
            let cmd = le32(off)
            let size = le32(off + 4)
            if cmd == 0x1d { sigFileOff = le32(off + 8) }  // LC_CODE_SIGNATURE.dataoff
            if size <= 0 { break }
            off += size
        }
        guard sigFileOff > 0, sigFileOff < data.count,
              be32(sigFileOff) == 0xfade0cc0 else {
            return "无内嵌代码签名(LC_CODE_SIGNATURE 不可用)"
        }

        let count = min(be32(sigFileOff + 8), 32)
        var entPlist = ""
        var derFound = false
        for i in 0..<count {
            let slot = be32(sigFileOff + 12 + i * 8)
            let blobOff = sigFileOff + be32(sigFileOff + 12 + i * 8 + 4)
            if slot == 5, be32(blobOff) == 0xfade7171 {  // CSMAGIC_EMBEDDED_ENTITLEMENTS
                let len = be32(blobOff + 4)
                if len > 8, blobOff + len <= data.count {
                    let lo = data.index(data.startIndex, offsetBy: blobOff + 8)
                    let hi = data.index(data.startIndex, offsetBy: blobOff + len)
                    entPlist = String(decoding: data[lo..<hi], as: UTF8.self)
                }
            } else if slot == 7 {
                derFound = true
            }
        }

        if entPlist.isEmpty {
            return "legacy entitlements 槽位缺失! der=\(derFound ? "有" : "无")"
        }
        let appID = entPlist.contains("TROLLTROLL.com.prism.proxy.PrismVPN") ? "ok" : "异常"
        let ne = entPlist.contains("packet-tunnel-provider") ? "ok" : "缺"
        let group = entPlist.contains("group.com.prism.proxy") ? "ok" : "缺"
        return "appID=\(appID) ne=\(ne) group=\(group) der=\(derFound ? "有" : "无")"
    }

    /// 用系统 Security 框架的【私有但在 iOS 真实导出】的 SecStaticCode API，
    /// 对 appex【整个 bundle 目录】做不启动扩展的静态校验。这与 pkd /
    /// pluginkit 注册扩展时所做的校验是同一套代码，因此能给出真机上的
    /// 权威结论：
    ///   staticCheck=valid     → 签名与 seal 都合法，问题不在静态校验，
    ///                          而在 pkd/lsd 注册、容器或开发者模式；
    ///   staticCheck=invalid(N)→ 系统真的拒绝了该 appex，N 为 OSStatus，
    ///                          可据此精确定位（资源 seal 坏、签名格式错等）。
    /// 这些符号不在公开 SDK 头里（Swift 直接写会"cannot find"），但
    /// TrollStore 自己（Shared/TSUtil.m）就在设备上直接链接使用它们，
    /// 故运行时 dlsym 一定能取到。
    static func diagnoseAppexStaticCode() -> String {
        guard let pluginsURL = Bundle.main.builtInPlugInsURL,
              let entries = try? FileManager.default.contentsOfDirectory(
                atPath: pluginsURL.path),
              let appexName = entries.first(where: { $0.hasSuffix(".appex") })
        else { return "appex未定位" }
        let appexURL = pluginsURL.appendingPathComponent(appexName)

        guard let sec = dlopen(
            "/System/Library/Frameworks/Security.framework/Security", 1
        ) else { return "Security dlopen失败" }

        guard let createP = dlsym(sec, "SecStaticCodeCreateWithPathAndAttributes"),
              let checkP = dlsym(sec, "SecStaticCodeCheckValidity"),
              let checkErrP = dlsym(sec, "SecStaticCodeCheckValidityWithErrors"),
              let infoP = dlsym(sec, "SecCodeCopySigningInformation")
        else { return "SecStaticCode符号未找到" }

        // 精确 ABI：OSStatus 即 Int32。所有 CF 对象参数统一用 CFTypeRef?
        // （OpaquePointer? 在 Swift ARM64 ABI 下与 CFTypeRef? 等价；但对
        //  CFDictionary attributes 必须用 CFTypeRef?，否则 Optional<CFDictionary>
        //  的 ABI 与 Optional<CFTypeRef> 不一致会导致调用约定错）。
        typealias CreateFn = @convention(c) (
            CFURL, UInt32, CFTypeRef?,
            UnsafeMutablePointer<CFTypeRef?>
        ) -> OSStatus
        typealias CheckFn = @convention(c) (
            OpaquePointer, UInt32, OpaquePointer?
        ) -> OSStatus
        // SecStaticCodeCheckValidityWithErrors(code, flags, requirement, &cfError)
        typealias CheckErrFn = @convention(c) (
            OpaquePointer, UInt32, OpaquePointer?,
            UnsafeMutablePointer<CFErrorRef?>
        ) -> OSStatus
        typealias InfoFn = @convention(c) (
            OpaquePointer, UInt32,
            UnsafeMutablePointer<CFTypeRef?>
        ) -> OSStatus

        // 指向 bundle【目录】：Security 会解析到主二进制并同时校验
        // _CodeSignature/CodeResources 资源封印，等价于 pkd 的整包校验。
        var codeRef: CFTypeRef?
        let create = unsafeBitCast(createP, to: CreateFn.self)
        let cs = create(appexURL as CFURL, 0, nil, &codeRef)
        guard cs == 0, let code = codeRef else {
            return "staticCreate失败=\(cs)"
        }
        let codePtr = unsafeBitCast(code, to: OpaquePointer.self)

        // 先用 CheckValidity，第三参 requirement=nil（OpaquePointer?）。
        let check = unsafeBitCast(checkP, to: CheckFn.self)
        let vs = check(codePtr, 0, nil)

        // 若失败，再用 CheckValidityWithErrors 拿 CFError 看具体原因。
        var errDetail = ""
        if vs != 0 {
            let checkErr = unsafeBitCast(checkErrP, to: CheckErrFn.self)
            var cfErr: CFErrorRef?
            let vs2 = checkErr(codePtr, 0, nil, &cfErr)
            errDetail = " errcode2=\(vs2)"
            // CFErrorRef 可无条件桥接到 NSError
            if let e = cfErr {
                let ns = e as NSError
                errDetail += " errDomain=\(ns.domain) errCode=\(ns.code) errMsg=\(ns.localizedDescription)"
            }
        }

        // 顺带读回 identifier / team / flags，便于交叉验证
        let info = unsafeBitCast(infoP, to: InfoFn.self)
        var infoDictRef: CFTypeRef?
        let infoStatus = info(codePtr, 0, &infoDictRef)
        var extra = ""
        if infoStatus == 0, let d = infoDictRef {
            let nd = d as NSDictionary
            if let ident = nd["identifier"] { extra += " id=\(ident)" }
            if let team = nd["teamidentifier"] { extra += " team=\(team)" }
            if let flags = nd["flags"] { extra += " flags=\(flags)" }
        }
        // 注意：Swift ARC 自动管理 CF 对象，不能也无需手动 CFRelease。

        let verdict = vs == 0 ? "valid" : "invalid(\(vs))"
        return "staticCheck=\(verdict)\(errDetail)\(extra)"
    }

    /// 打包诊断：列出 PlugIns 下每个 appex 的 bundle ID 与扩展点。
    /// 随启动失败错误一起输出，用户截图即可远程定位是 IPA 缺扩展
    /// 还是系统/配置层问题
    static func diagnosePackaging() -> String {
        guard let pluginsURL = Bundle.main.builtInPlugInsURL else {
            return "PlugIns 目录不存在(builtInPlugInsURL=nil)"
        }
        let entries = (try? FileManager.default.contentsOfDirectory(
            atPath: pluginsURL.path)) ?? []
        let appexes = entries.filter { $0.hasSuffix(".appex") }
        if appexes.isEmpty {
            return "PlugIns 下无 .appex(扩展未打包!)"
        }
        return appexes.map { name in
            let bundle = Bundle(url: pluginsURL.appendingPathComponent(name))
            let bid = bundle?.infoDictionary?["CFBundleIdentifier"] as? String ?? "?"
            let point = ((bundle?.infoDictionary?["NSExtension"] as? [String: Any])?["NSExtensionPointIdentifier"] as? String) ?? "?"
            let ver = bundle?.infoDictionary?["CFBundleVersion"] as? String ?? "?"
            return "\(name): id=\(bid) point=\(point) extbuild=\(ver)"
        }.joined(separator: " | ")
    }

    // MARK: - 强制注册扩展

    /// 保存 LSRegisterURL 和 pluginkit 的结果，供诊断输出
    static var lsRegisterResult: String = "未调用"
    static var pluginkitResult: String = "未调用"
    static var lsAppWorkspaceResult: String = "未调用"

    /// 强制向 Launch Services 注册 appex bundle。
    /// TrollStore 安装后 pluginkit 可能未注册扩展
    ///（CoreTrust 绕过只影响 FrontBoard，pluginkit 有独立验证），
    /// 导致系统找不到扩展 → NEVPNConnectionErrorDomain code=14。
    /// 方案1: dlsym 多路径加载 LSRegisterURL（含下划线前缀）
    /// 方案2: LSApplicationWorkspace 私有 API（TrollStore 允许）
    /// 方案3: posix_spawn pluginkit -a（需逃逸沙箱，通常失败）
    static func registerAppexWithLaunchServices() {
        guard let pluginsURL = Bundle.main.builtInPlugInsURL,
              let entries = try? FileManager.default.contentsOfDirectory(
                atPath: pluginsURL.path)
        else {
            NSLog("[PrismVPN] registerAppex: PlugIns 目录不存在")
            lsRegisterResult = "PlugIns不存在"
            return
        }
        for entry in entries where entry.hasSuffix(".appex") {
            let appexURL = pluginsURL.appendingPathComponent(entry)
            let appexPath = appexURL.path
            let csPath = appexURL.appendingPathComponent("_CodeSignature/CodeResources").path
            let hasSeal = FileManager.default.fileExists(atPath: csPath)
            NSLog("[PrismVPN] appex=\(entry) CodeResources=\(hasSeal ? "有" : "无")")

            // 方案1: dlsym 多框架/多符号名尝试
            // Apple 平台 C 符号在符号表中带下划线前缀：_LSRegisterURL
            let frameworks = [
                "/System/Library/Frameworks/CoreServices.framework/CoreServices",
                "/System/Library/PrivateFrameworks/MobileCoreServices.framework/MobileCoreServices",
                "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/LaunchServices"
            ]
            let symbolNames = ["_LSRegisterURL", "LSRegisterURL"]
            var lsDone = false
            for fwPath in frameworks {
                guard !lsDone,
                      let handle = dlopen(fwPath, 1)  // RTLD_LAZY
                else { continue }
                for symName in symbolNames {
                    guard !lsDone,
                          let sym = dlsym(handle, symName)
                    else { continue }
                    typealias Fn = @convention(c) (CFURL, Bool) -> Int32
                    let fn = unsafeBitCast(sym, to: Fn.self)
                    let status = fn(appexURL as CFURL, true)
                    lsRegisterResult = "\(symName)@\((fwPath as NSString).lastPathComponent)=\(status)"
                    NSLog("[PrismVPN] \(symName) status=\(status)")
                    lsDone = true
                }
                dlclose(handle)
            }
            if !lsDone {
                lsRegisterResult = "所有路径/符号均未找到"
                NSLog("[PrismVPN] LSRegisterURL not found in any framework")
            }

            // 方案2: LSApplicationWorkspace 私有 API
            // TrollStore CoreTrust 绕过允许调用私有类
            tryLSApplicationWorkspace(appexURL: appexURL)

            // 方案3: posix_spawn pluginkit -a（通常因沙箱失败）
            tryPluginkitRegister(appexPath: appexPath)
        }
    }

    /// 通过 LSApplicationWorkspace 私有 API 注册 app 及其 appex。
    ///
    /// TrollStore root helper（RootHelper/uicache.m 的 registerPath）证实，
    /// 注册 app 与其 PlugIns 的唯一正确 API 是：
    ///   -[LSApplicationWorkspace registerApplicationDictionary:]
    /// 该方法接收一个描述完整的字典；appex 通过字典里的
    /// `_LSBundlePlugins` 键（每个 plugin 标 ApplicationType=PluginKitPlugin）
    /// 一并注册到 pluginkit。旧代码猜的 registerApplicationWithBundleID:atURL:
    /// 在 iOS 上并不存在（诊断"bid:url 不可响应"）。
    private static func tryLSApplicationWorkspace(appexURL: URL) {
        guard let wsAnyClass = NSClassFromString("LSApplicationWorkspace")
        else {
            NSLog("[PrismVPN] LSApplicationWorkspace class not found")
            lsAppWorkspaceResult = "class未找到"
            return
        }
        // NSClassFromString 返回 AnyClass（元类型），cast AnyObject 后
        // 才能调用 perform/responds（根元类继承自 NSObject）。
        let wsClass = wsAnyClass as AnyObject
        let defaultSel = NSSelectorFromString("defaultWorkspace")
        guard wsClass.responds(to: defaultSel),
              let workspace = wsClass.perform(defaultSel)?.takeUnretainedValue()
        else {
            NSLog("[PrismVPN] defaultWorkspace not available")
            lsAppWorkspaceResult = "defaultWorkspace不可用"
            return
        }

        let regSel = NSSelectorFromString("registerApplicationDictionary:")
        guard workspace.responds(to: regSel) else {
            NSLog("[PrismVPN] registerApplicationDictionary: not responds")
            lsAppWorkspaceResult = "regDict不可响应"
            return
        }

        let mainBid = Bundle.main.bundleIdentifier ?? "com.prism.proxy"
        let appexBid = extensionBundleIdentifier
        let mainEnt = knownMainEntitlements()
        let appexEnt = knownAppexEntitlements()

        // App Group 共享容器（主 App 与 appex 同一组）
        var groupContainers = [String: String]()
        if let gURL = FileManager.default.containerURL(
            forSecurityApplicationGroupIdentifier: prismAppGroupID) {
            groupContainers[prismAppGroupID] = gURL.path
        }

        // ---- appex（PluginKitPlugin）注册字典 ----
        var plugin: [String: Any] = [
            "ApplicationType": "PluginKitPlugin",
            "CFBundleIdentifier": appexBid,
            "CodeInfoIdentifier": appexBid,
            "CompatibilityState": 0,
            "IsContainerized": true,
            "Path": appexURL.path,
            "PluginOwnerBundleID": mainBid,
            "SignerOrganization": "Apple Inc.",
            "SignatureVersion": 132352,
            "SignerIdentity": "Apple iPhone OS Application Signing",
            "IsAdHocSigned": true,
            "Entitlements": appexEnt,
        ]
        if !groupContainers.isEmpty {
            plugin["HasAppGroupContainers"] = true
            plugin["GroupContainers"] = groupContainers
        }

        // ---- 主 App 注册字典 ----
        var dict: [String: Any] = [
            "ApplicationType": "User",
            "CFBundleIdentifier": mainBid,
            "CodeInfoIdentifier": mainBid,
            "CompatibilityState": 0,
            "IsContainerized": true,
            "Container": NSHomeDirectory(),
            "IsDeletable": true,
            "Path": Bundle.main.bundlePath,
            "SignerOrganization": "Apple Inc.",
            "SignatureVersion": 132352,
            "SignerIdentity": "Apple iPhone OS Application Signing",
            "IsAdHocSigned": true,
            "LSInstallType": 1,
            "HasMIDBasedSINF": 0,
            "MissingSINF": 0,
            "FamilyID": 0,
            "IsOnDemandInstallCapable": 0,
            "Entitlements": mainEnt,
            "_LSBundlePlugins": [appexBid: plugin],
        ]
        if !groupContainers.isEmpty {
            dict["HasAppGroupContainers"] = true
            dict["GroupContainers"] = groupContainers
        }

        // registerApplicationDictionary: 返回 BOOL（非对象），perform 无法
        // 可靠取回；而 Swift SDK 将 objc_msgSend 标记为 variadic 不可直接用。
        // 故通过 dlopen(NULL)/dlsym 运行时拿到 objc_msgSend 指针（libobjc
        // 已加载，必能找到），再 unsafeBitCast 成 1 参函数指针精确调用。
        typealias RegFn = @convention(c) (AnyObject, Selector, NSDictionary) -> Bool
        let h = dlopen(nil, 1) // RTLD_LAZY，NULL 返回全局作用域
        if let sym = dlsym(h, "objc_msgSend") {
            let fn = unsafeBitCast(sym, to: RegFn.self)
            let ok = fn(workspace, regSel, dict as NSDictionary)
            lsAppWorkspaceResult = "regDict=\(ok)"
            NSLog("[PrismVPN] registerApplicationDictionary: ret=\(ok)")
        } else {
            // dlsym 失败则退回 perform（调用仍会发生，只是拿不到 BOOL）
            workspace.perform(regSel, with: dict as NSDictionary)
            lsAppWorkspaceResult = "regDict已调用(perform)"
            NSLog("[PrismVPN] registerApplicationDictionary: invoked via perform")
        }
    }

    /// 主 App 应有的 entitlements（与 CI 签入的 TrollStore.entitlements.plist
    /// 一致）。iOS 上 SecTask* 是未导出的 SPI（Swift 无法引用），故按已知
    /// 值构造。
    private static func knownMainEntitlements() -> [String: Any] {
        let mainBid = Bundle.main.bundleIdentifier ?? "com.prism.proxy"
        return [
            "application-identifier": "TROLLTROLL.\(mainBid)",
            "com.apple.developer.team-identifier": "TROLLTROLL",
            "com.apple.developer.networking.networkextension":
                ["packet-tunnel-provider"],
            "com.apple.security.application-groups": [prismAppGroupID],
            "keychain-access-groups": ["TROLLTROLL.\(mainBid)"],
        ]
    }

    /// appex 应有的 entitlements（与 CI 签入的 PrismVPN.entitlements.plist
    /// 一致）。appex 在独立进程，故按已知值构造。
    private static func knownAppexEntitlements() -> [String: Any] {
        let appexBid = extensionBundleIdentifier
        return [
            "application-identifier": "TROLLTROLL.\(appexBid)",
            "com.apple.developer.team-identifier": "TROLLTROLL",
            "com.apple.developer.networking.networkextension":
                ["packet-tunnel-provider"],
            "com.apple.security.application-groups": [prismAppGroupID],
            "keychain-access-groups": ["TROLLTROLL.\(appexBid)"],
        ]
    }

    /// 通过 posix_spawn 调用 pluginkit -a 强制注册扩展
    private static func tryPluginkitRegister(appexPath: String) {
        var pid: pid_t = 0
        var argv: [UnsafeMutablePointer<CChar>?] = [
            strdup("pluginkit"),
            strdup("-a"),
            strdup(appexPath),
            nil
        ]
        defer { argv.prefix(while: { $0 != nil }).forEach { free($0) } }

        let spawnResult = posix_spawn(&pid, "/usr/bin/pluginkit", nil, nil, &argv, nil)
        if spawnResult == 0 {
            var status: Int32 = 0
            waitpid(pid, &status, 0)
            pluginkitResult = "pid=\(pid) exit=\(status)"
            NSLog("[PrismVPN] pluginkit -a exit=\(status)")
        } else {
            pluginkitResult = "spawn=\(spawnResult)"
            NSLog("[PrismVPN] posix_spawn pluginkit failed: \(spawnResult) (errno)")
        }
    }

    // MARK: 失效配置识别

    /// 判断错误是否表示「系统保存的 VPN 配置已找不到对应扩展」。
    ///
    /// App 覆盖安装（尤其是 TrollStore 重签/升级安装）后，系统 Network
    /// Extension 偏好里的旧配置与新装 appex 的内部映射可能失效——即使
    /// bundle ID 表面一致，继续在旧 manager 上 save/start 只会反复报：
    ///   NEVPNConnectionErrorDomain code=14
    ///   "The VPN app used by the VPN configuration is not installed"
    /// 唯一可靠的恢复方式是 removeFromPreferences 清除旧配置，
    /// 再用全新的 NETunnelProviderManager 重建。
    static func isStaleOrInvalid(_ error: Error) -> Bool {
        var current: NSError? = error as NSError
        while let ns = current {
            // NEVPNConnectionErrorDomain 14：配置指向的扩展未安装
            if ns.domain == "NEVPNConnectionErrorDomain" && ns.code == 14 {
                return true
            }
            // NEVPNErrorDomain 1 = configurationInvalid（配置失效）
            if ns.domain == "NEVPNErrorDomain" && ns.code == 1 {
                return true
            }
            // 本控制器 code=-3：status=invalid，系统中无有效持久化配置，
            // 重建 manager 即可恢复
            if ns.domain == "PrismVPN" && ns.code == -3 {
                return true
            }
            // 「无任何错误痕迹的立即断开」：既没有扩展写入的 prism_error.txt，
            // 系统也没有断开错误记录（iOS14/15 无 fetchLastDisconnectError，
            // 或 iOS16+ 返回 nil）。扩展 startTunnel 只要真正跑起来，任何
            // 失败都会写共享错误文件——所以这种情形等价于扩展根本没被
            // 系统拉起（旧系统上的 code=14），同样需要清除重建
            if ns.domain == "PrismVPN", ns.code == -2,
               ns.localizedDescription.hasPrefix("扩展启动后立即断开")
            {
                return true
            }
            // 文案兜底（不同 iOS 版本 domain 偶有差异）
            if ns.localizedDescription.lowercased().contains("not installed") {
                return true
            }
            current = ns.userInfo[NSUnderlyingErrorKey] as? NSError
        }
        return false
    }

    // MARK: 启动（带失效配置自愈，最多重试 1 次）

    func start(configPath: String, completion: @escaping (Error?) -> Void) {
        startAttempt(configPath: configPath, attemptsRemaining: 1, completion: completion)
    }

    private func startAttempt(
        configPath: String,
        attemptsRemaining: Int,
        completion: @escaping (Error?) -> Void
    ) {
        // code=14 场景下扩展根本没被系统拉起，没有机会执行扩展端的
        // clearSharedError；主 App 启动前先清，避免读到上一轮残留的
        // 陈旧 prism_error.txt 而误判断开原因
        clearSharedErrorFile()

        // 【关键修复】TrollStore 的 CoreTrust 绕过让 FrontBoard 接受主 App，
        // 但 pluginkit（扩展注册守护进程）可能有自己的独立验证。如果
        // pluginkit 未注册 appex，系统不知道扩展存在 → Code 14。
        // 通过 dlsym 动态加载 LSRegisterURL，强制向 Launch Services 注册
        // appex bundle，触发 pluginkit 扫描注册。
        PrismTunnelController.registerAppexWithLaunchServices()

        // 主动 pre-clean + 版本同步（同时解决三个问题）：
        //
        // 1) 解决"Prism - 需要更新"红色提示：iOS 在 VPN 配置里记录了
        //    App 安装时的版本号/签名时间戳；每次 TrollStore 重装会更新
        //    签名但 VPN 配置里的版本号不自动同步，iOS 因此认为配置
        //    指向"过时"的 App。loadOrCreateManager 找到已存配置后，
        //    主动 saveToPreferences 一次即可让 iOS 把当前 App 的真实
        //    版本号/时间戳写进 VPN 配置，消除系统设置里的红色提示
        //
        // 2) 清理旧 TrollStore 重装留下的残留条目：providerBundleIdentifier
        //    是老签名/老版本的 .PrismVPN，新 App loadAllFromPreferences
        //    返回空数组（配置指向的扩展 App ID 与签名不匹配），但系统
        //    设置里残留为只读条目，用户看起来就是"需要 2 次添加"
        //
        // 3) 确保第一次 saveToPreferences 时 providerBundleIdentifier
        //    与当前真实 appex 完全一致——万一 discoverExtensionBundleID
        //    失败回落硬编码 fallback，这里也能二次校验
        preCleanAndSyncVersion {
            self.loadOrCreateManager { result in
                switch result {
                case .failure(let error):
                    self.recoverOrFinish(
                        error, configPath: configPath,
                        attemptsRemaining: attemptsRemaining, completion: completion
                    )
                case .success(let manager):
                    self.configureSaveAndStart(
                        manager: manager, configPath: configPath,
                        attemptsRemaining: attemptsRemaining, completion: completion
                    )
                }
            }
        }
    }

    /// 启动前一次性预处理：遍历全部 VPN 配置，
    /// - 旧签名/旧 bundle 的 Prism 条目 → removeFromPreferences
    /// - 找到与当前 extensionBundleIdentifier 匹配的条目 → 主动 saveToPreferences
    ///   让 iOS 把当前 App 版本号/签名时间戳同步进 VPN 配置（消除"需要更新"提示）
    private func preCleanAndSyncVersion(completion: @escaping () -> Void) {
        NETunnelProviderManager.loadAllFromPreferences { managers, error in
            guard let managers = managers else {
                if let error = error {
                    NSLog("[PrismVPN] preClean loadAllFromPreferences 失败: \(error.localizedDescription)")
                }
                completion()
                return
            }
            let group = DispatchGroup()
            var matched: NETunnelProviderManager?
            for manager in managers {
                guard let proto = manager.protocolConfiguration
                    as? NETunnelProviderProtocol else { continue }
                let isOurs = proto.providerBundleIdentifier?.hasSuffix(".PrismVPN") == true
                guard isOurs else { continue }
                if proto.providerBundleIdentifier == extensionBundleIdentifier {
                    matched = manager
                } else {
                    // 旧签名/旧 bundle ID 的残留条目 → 删除
                    NSLog("[PrismVPN] 删除旧签名残留配置: \(proto.providerBundleIdentifier ?? "?")")
                    group.enter()
                    manager.removeFromPreferences { _ in group.leave() }
                }
            }
            // 找到匹配条目 → 主动 saveToPreferences，
            // 让 iOS 同步当前 App 版本号（消除"需要更新"红色提示）
            if let matched = matched {
                NSLog("[PrismVPN] 主动 save 同步 VPN 配置版本号")
                group.enter()
                matched.saveToPreferences { _ in group.leave() }
            }
            group.notify(queue: .main) { completion() }
        }
    }

    /// 失败出口：若属于失效配置且还有重试机会，清空全部 manager 后重建；
    /// 否则直接把错误交回上层
    private func recoverOrFinish(
        _ error: Error,
        configPath: String,
        attemptsRemaining: Int,
        completion: @escaping (Error?) -> Void
    ) {
        guard attemptsRemaining > 0, PrismTunnelController.isStaleOrInvalid(error) else {
            completion(error)
            return
        }
        NSLog("[PrismVPN] 检测到失效 VPN 配置，清除全部 manager 后重建重试: \(PrismTunnelController.describe(error: error))")
        resetAllManagers {
            self.startAttempt(
                configPath: configPath,
                attemptsRemaining: attemptsRemaining - 1,
                completion: completion
            )
        }
    }

    private func configureSaveAndStart(
        manager: NETunnelProviderManager,
        configPath: String,
        attemptsRemaining: Int,
        completion: @escaping (Error?) -> Void
    ) {
        let proto = NETunnelProviderProtocol()
        proto.providerBundleIdentifier = extensionBundleIdentifier
        proto.serverAddress = managerServerTag
        proto.disconnectOnSleep = false
        // 只传路径（短），避开 NETunnelProviderManager
        // providerConfiguration 512KB 限制
        proto.providerConfiguration = ["config_path": configPath]

        manager.protocolConfiguration = proto
        manager.isEnabled = true

        manager.saveToPreferences { saveError in
            if let saveError = saveError {
                NSLog("[PrismVPN] saveToPreferences 失败: \(PrismTunnelController.describe(error: saveError))")
                self.recoverOrFinish(
                    saveError, configPath: configPath,
                    attemptsRemaining: attemptsRemaining, completion: completion
                )
                return
            }
            NSLog("[PrismVPN] saveToPreferences 成功, extBundleId=\(extensionBundleIdentifier)")

            // 关键修复（TrollStore 环境 code=14 根因）：
            // saveToPreferences completion 只保证偏好文件写入磁盘，
            // 但 nesessionmanager 异步处理配置变更（校验扩展 bundle、
            // 注册路径、加载签名），紧接着的 startVPNTunnel 会触发
            // 系统扩展校验，此时 bundle 还没注册 → code=14
            // "The VPN app used by the VPN configuration is not installed"。
            //
            // 正确做法：等 NEVPNConfigurationChangeNotification
            // （系统完成配置变更处理后发出）再 reload+start；
            // 2s 兜底定时器防止通知丢失（极端 iOS 版本）。
            self.waitForConfigChangeThenStart(
                attemptsRemaining: attemptsRemaining,
                completion: completion
            )
        }
    }

    /// 等待系统配置变更通知后，再 reload 并 startVPNTunnel。
    /// 2 秒内若通知未到，直接兜底 start（正常情况下 save 后配置应已生效）
    private func waitForConfigChangeThenStart(
        attemptsRemaining: Int,
        completion: @escaping (Error?) -> Void
    ) {
        var observer: NSObjectProtocol?
        var fired = false
        var timer: Timer?

        let fireOnce: () -> Void = { [weak self] in
            guard !fired, let self = self else { return }
            fired = true
            if let o = observer {
                NotificationCenter.default.removeObserver(o)
            }
            timer?.invalidate()
            self.reloadAndStart(
                attemptsRemaining: attemptsRemaining,
                completion: completion
            )
        }

        observer = NotificationCenter.default.addObserver(
            forName: NSNotification.Name.NEVPNConfigurationChange,
            object: nil,
            queue: .main,
            using: { _ in
                NSLog("[PrismVPN] 收到 NEVPNConfigurationChange，准备 reload+start")
                fireOnce()
            }
        )

        // 兜底：2 秒内系统未发变更通知（如首次 save 后通知不触发的
        // 边缘场景），直接走 reload+start。总等待时间 2s，不会明显拖慢
        // 正常启动路径（正常 save 后通知应在 100~300ms 内到达）
        timer = Timer.scheduledTimer(withTimeInterval: 2.0, repeats: false) { _ in
            NSLog("[PrismVPN] NEVPNConfigurationChange 未在 2s 内到达，兜底 reload+start")
            fireOnce()
        }
        RunLoop.main.add(timer!, forMode: .common)
    }

    /// reload（loadAllFromPreferences）找到持久化配置后发起 startVPNTunnel
    private func reloadAndStart(
        attemptsRemaining: Int,
        completion: @escaping (Error?) -> Void
    ) {
        self.reloadByServerTag { reloadResult in
            switch reloadResult {
            case .failure(let error):
                NSLog("[PrismVPN] reload 失败: \(PrismTunnelController.describe(error: error))")
                self.recoverOrFinish(
                    error, configPath: "",
                    attemptsRemaining: attemptsRemaining, completion: completion
                )
            case .success(let saved):
                do {
                    try saved.connection.startVPNTunnel()
                    NSLog("[PrismVPN] startVPNTunnel 调用成功")

                    self.waitForConnected(connection: saved.connection, timeout: 20) { err in
                        if let err = err {
                            self.recoverOrFinish(
                                err, configPath: "",
                                attemptsRemaining: attemptsRemaining, completion: completion
                            )
                        } else {
                            completion(nil)
                        }
                    }
                } catch {
                    NSLog("[PrismVPN] startVPNTunnel 失败: \(PrismTunnelController.describe(error: error))")
                    self.recoverOrFinish(
                        error, configPath: "",
                        attemptsRemaining: attemptsRemaining, completion: completion
                    )
                }
            }
        }
    }

    /// 简化版 reload：按 server tag（"Prism"）查找持久化配置，
    /// 供 waitForConfigChangeThenStart 使用（此时已知 save 完成，
    /// 不再需要在 reload 里校验 providerBundleIdentifier——
    /// save 时已正确设置）
    private func reloadByServerTag(
        completion: @escaping (Result<NETunnelProviderManager, Error>) -> Void
    ) {
        NETunnelProviderManager.loadAllFromPreferences { managers, error in
            if let error = error {
                completion(.failure(error))
                return
            }
            let saved = managers?.first {
                ($0.protocolConfiguration as? NETunnelProviderProtocol)?
                    .serverAddress == managerServerTag
            }
            if let saved = saved {
                NSLog("[PrismVPN] reload 找到持久化配置")
                completion(.success(saved))
            } else {
                completion(.failure(NSError(
                    domain: "NEVPNErrorDomain", code: 1,
                    userInfo: [NSLocalizedDescriptionKey: "reload 未找到持久化 VPN 配置"]
                )))
            }
        }
    }

    /// 监听 NEVPNStatusDidChange，直到 connected / disconnected / 超时
    private func waitForConnected(
        connection: NEVPNConnection,
        timeout: TimeInterval,
        completion: @escaping (Error?) -> Void
    ) {
        var observer: NSObjectProtocol?
        var completed = false
        var timer: Timer?

        let finish: (Error?) -> Void = { error in
            guard !completed else { return }
            completed = true
            if let o = observer {
                NotificationCenter.default.removeObserver(o)
            }
            timer?.invalidate()
            completion(error)
        }

        // 立即检查当前状态
        if connection.status == .connected {
            NSLog("[PrismVPN] 状态已是 connected")
            finish(nil)
            return
        }

        observer = NotificationCenter.default.addObserver(
            forName: NSNotification.Name.NEVPNStatusDidChange,
            object: connection,
            queue: .main
        ) { note in
            let status = connection.status
            NSLog("[PrismVPN] NEVPNStatusDidChange: \(status.rawValue)")
            switch status {
            case .connected:
                finish(nil)
            case .disconnected:
                // 解析真实断开原因（共享错误文件 → iOS16 系统 API → 通用描述）
                self.describeDisconnect(connection: connection) { desc, underlying in
                    // 底层 NSError 经 NSUnderlyingErrorKey 透传，
                    // 供 isStaleOrInvalid 识别 code=14 触发重建重试
                    var userInfo: [String: Any] = [
                        NSLocalizedDescriptionKey: desc
                    ]
                    if let underlying = underlying {
                        userInfo[NSUnderlyingErrorKey] = underlying
                    }
                    finish(NSError(
                        domain: "PrismVPN", code: -2,
                        userInfo: userInfo
                    ))
                }
            case .invalid:
                finish(NSError(
                    domain: "PrismVPN", code: -3,
                    userInfo: [NSLocalizedDescriptionKey: "VPN 配置无效（status=invalid）"]
                ))
            default:
                break // connecting / reasserting / disconnecting 继续等
            }
        }

        // 超时计时器
        timer = Timer.scheduledTimer(withTimeInterval: timeout, repeats: false) { _ in
            finish(NSError(
                domain: "PrismVPN", code: -4,
                userInfo: [NSLocalizedDescriptionKey:
                    "VPN 启动超时（\(Int(timeout))s），扩展未进入 connected 状态；当前 status=\(connection.status.rawValue)"]
            ))
        }
        RunLoop.main.add(timer!, forMode: .common)
    }

    /// 解析 VPN 断开原因，三级回退：
    /// 1. App Group「prism_error.txt」：扩展 startTunnel 失败时写入，
    ///    含内核返回的精确错误原文，兼容所有 iOS 版本
    /// 2. iOS 16+ 系统 API fetchLastDisconnectError（扩展进程崩溃等
    ///    来不及写文件的场景；code=14 也只在这里出现）
    /// 3. 通用描述
    ///
    /// 同时回传底层系统 NSError（若有），供上层做失效配置分类。
    private func describeDisconnect(
        connection: NEVPNConnection,
        completion: @escaping (String, Error?) -> Void
    ) {
        let generic = "扩展启动后立即断开（status=disconnected）"

        // 1. 共享错误文件
        if let groupURL = FileManager.default.containerURL(
            forSecurityApplicationGroupIdentifier: prismAppGroupID
        ) {
            let errFile = groupURL.appendingPathComponent("prism_error.txt")
            if let text = try? String(contentsOf: errFile, encoding: .utf8),
               !text.isEmpty
            {
                NSLog("[PrismVPN] 读到共享错误: \(text)")
                completion(text, nil)
                return
            }
        }

        // 2. iOS 16+ 系统 API。completionHandler 可能在后台线程回调，
        // 统一切回主线程，避免与超时计时器产生 finish 竞态
        if #available(iOS 16, *) {
            connection.fetchLastDisconnectError { err in
                let desc = err.map {
                    PrismTunnelController.describe(error: $0)
                } ?? generic
                if let err = err {
                    NSLog("[PrismVPN] fetchLastDisconnectError: \(desc)")
                }
                DispatchQueue.main.async { completion(desc, err) }
            }
        } else {
            // 3. iOS 14/15 无系统 API 且扩展未写文件（如进程被系统直接杀死）
            completion(generic, nil)
        }
    }

    func stop(completion: @escaping (Error?) -> Void) {
        loadOrCreateManager { result in
            switch result {
            case .failure(let error):
                completion(error)
            case .success(let manager):
                manager.connection.stopVPNTunnel()
                // 等待 disconnected 确认（与 start 对称）
                self.waitForDisconnected(connection: manager.connection, timeout: 10) { _ in
                    completion(nil)
                }
            }
        }
    }

    /// 监听 NEVPNStatusDidChange，直到 disconnected / invalid / 超时
    private func waitForDisconnected(
        connection: NEVPNConnection,
        timeout: TimeInterval,
        completion: @escaping (Error?) -> Void
    ) {
        var observer: NSObjectProtocol?
        var completed = false
        var timer: Timer?

        let finish: (Error?) -> Void = { error in
            guard !completed else { return }
            completed = true
            if let o = observer { NotificationCenter.default.removeObserver(o) }
            timer?.invalidate()
            completion(error)
        }

        if connection.status == .disconnected || connection.status == .invalid {
            finish(nil)
            return
        }

        observer = NotificationCenter.default.addObserver(
            forName: NSNotification.Name.NEVPNStatusDidChange,
            object: connection,
            queue: .main
        ) { _ in
            let status = connection.status
            NSLog("[PrismVPN] stop status change: \(status.rawValue)")
            if status == .disconnected || status == .invalid {
                finish(nil)
            }
        }

        timer = Timer.scheduledTimer(withTimeInterval: timeout, repeats: false) { _ in
            finish(NSError(
                domain: "PrismVPN", code: -5,
                userInfo: [NSLocalizedDescriptionKey: "VPN 停止超时（\(Int(timeout))s）"]
            ))
        }
        RunLoop.main.add(timer!, forMode: .common)
    }

    // MARK: - Manager 持久化

    private func loadOrCreateManager(
        completion: @escaping (Result<NETunnelProviderManager, Error>) -> Void
    ) {
        NETunnelProviderManager.loadAllFromPreferences { managers, error in
            if let error = error {
                NSLog("[PrismVPN] loadAllFromPreferences 失败: \(PrismTunnelController.describe(error: error))")
                completion(.failure(error))
                return
            }
            let all = managers ?? []
            PrismTunnelController.lastManagerCount = all.count
            NSLog("[PrismVPN] loadAllFromPreferences 成功, 找到 \(all.count) 个 manager")

            // 主动清理（覆盖升级后 code=14 的第一道防线）：
            // - keep：属于本 App 且 providerBundleIdentifier 与当前
            //   预期扩展完全一致的 manager，只保留一个
            // - trash：provider 已变更（旧主 App ID / 重复 manager），
            //   继续使用必然 code=14，先删除
            var keep: NETunnelProviderManager?
            var trash: [NETunnelProviderManager] = []
            for manager in all {
                let proto = manager.protocolConfiguration as? NETunnelProviderProtocol
                let isOurs = proto?.serverAddress == managerServerTag
                let providerID = proto?.providerBundleIdentifier
                if isOurs, providerID == extensionBundleIdentifier {
                    if keep == nil {
                        keep = manager
                    } else {
                        trash.append(manager) // 重复配置
                    }
                } else if isOurs
                    || providerID?.hasSuffix(".PrismVPN") == true
                {
                    trash.append(manager)
                }
            }

            let provide: () -> Void = {
                if let keep = keep {
                    completion(.success(keep))
                } else {
                    completion(.success(NETunnelProviderManager()))
                }
            }

            guard !trash.isEmpty else {
                provide()
                return
            }
            NSLog("[PrismVPN] 发现 \(trash.count) 个失效/重复 VPN 配置，删除中")
            let group = DispatchGroup()
            for manager in trash {
                group.enter()
                manager.removeFromPreferences { removeError in
                    if let removeError = removeError {
                        NSLog("[PrismVPN] removeFromPreferences 失败: \(PrismTunnelController.describe(error: removeError))")
                    }
                    group.leave()
                }
            }
            group.notify(queue: .main, execute: provide)
        }
    }

    /// 删除 App Group 中的 prism_error.txt（主 App 侧清理）。
    /// code=14 场景下扩展进程根本不会启动，扩展端 clearSharedError
    /// 无法执行，必须由主 App 在每轮启动前主动清除。
    private func clearSharedErrorFile() {
        guard let groupURL = FileManager.default.containerURL(
            forSecurityApplicationGroupIdentifier: prismAppGroupID
        ) else { return }
        try? FileManager.default.removeItem(
            at: groupURL.appendingPathComponent("prism_error.txt")
        )
    }

    /// 无条件删除系统中本 App 的全部 VPN 配置（code=14 自愈用）
    private func resetAllManagers(_ completion: @escaping () -> Void) {
        NETunnelProviderManager.loadAllFromPreferences { managers, _ in
            let all = managers ?? []
            guard !all.isEmpty else {
                completion()
                return
            }
            let group = DispatchGroup()
            for manager in all {
                group.enter()
                manager.removeFromPreferences { removeError in
                    if let removeError = removeError {
                        NSLog("[PrismVPN] reset removeFromPreferences 失败: \(PrismTunnelController.describe(error: removeError))")
                    }
                    group.leave()
                }
            }
            group.notify(queue: .main) {
                NSLog("[PrismVPN] 已清除全部 \(all.count) 个残留 VPN 配置")
                completion()
            }
        }
    }

    private func reload(
        manager: NETunnelProviderManager,
        completion: @escaping (Result<NETunnelProviderManager, Error>) -> Void
    ) {
        NETunnelProviderManager.loadAllFromPreferences { managers, error in
            if let error = error {
                completion(.failure(error))
                return
            }
            // 严格匹配：server 标签 + 当前预期的扩展 bundle ID。
            // save 成功却找不到配置 → 按失效处理，交由上层重建重试，
            // 不再回落给 in-memory manager（其 connection 未持久化，
            // startVPNTunnel 必然失败）。
            let saved = managers?.first {
                guard let proto = $0.protocolConfiguration as? NETunnelProviderProtocol,
                      proto.serverAddress == managerServerTag
                else { return false }
                return proto.providerBundleIdentifier == extensionBundleIdentifier
            }
            if let saved = saved {
                completion(.success(saved))
            } else {
                completion(.failure(NSError(
                    domain: "NEVPNErrorDomain", code: 1,
                    userInfo: [NSLocalizedDescriptionKey:
                        "save 后未找到与 \(extensionBundleIdentifier) 匹配的持久化 VPN 配置"]
                )))
            }
        }
    }
}
