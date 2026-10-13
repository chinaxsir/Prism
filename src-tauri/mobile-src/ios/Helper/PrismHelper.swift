// PrismHelper — TrollStore 环境的随包 root helper（CLI 二进制）。
//
// 背景：TrollStore 安装不走 installd，NESM/pkd 对我们的 VPN 扩展始终
// 报 code=14（"The VPN app used by the VPN configuration is not
// installed"）。主 App 以 root persona（persona-mgmt + no-sandbox，
// rtEnt=platform=1 已真机验证）spawn 本工具，由本工具以 root 身份：
//   1. register — 用 LSApplicationWorkspace 强扭注册扩展
//      （registerApplicationDictionary: 完整官方字典 + registerPlugin:）
//   2. fixperm  — 修复 PlugIns 目录/文件属主与权限（mobile:mobile，
//      目录 0755、可执行 0755、其余 0644），防 nesessionmanager 读
//      沙盒时因权限污染拒绝
// 每一步结果打印到 stdout（形如 result=...），由主 App 捕获进诊断。
//
// 用法:
//   PrismHelper register <mainAppBundlePath> <appexBundlePath>
//   PrismHelper fixperm  <mainAppBundlePath> <appexBundlePath>
//
// 构建要求：动态链接系统 Swift stdlib（Xcode 16 起已移除
// -static-stdlib 对 Apple 平台的支持）。libswiftCore/Foundation 等
// 自 iOS 13 起在 dyld 共享缓存中，CI 侧以
// -rpath /usr/lib/swift（系统优先）+ -rpath @executable_path/Frameworks
// （兜底 back-deploy 的 _Concurrency）保证设备端解析。

import Darwin
import Foundation
import Security

let MOBILE_UID: uid_t = 501
let MOBILE_GID: gid_t = 501

// MARK: - entitlements 读取（SecTask API，读任意已签二进制）

func dumpEntitlements(from execPath: String) -> [String: Any] {
    var out = [String: Any]()
    let url = URL(fileURLWithPath: execPath) as CFURL
    guard let task = SecTaskCreateFromPath(nil, url, nil) else {
        FileHandle.standardError.write("helper: ent dump fail \(execPath)\n".data(using: .utf8)!)
        return out
    }
    let keys = [
        "application-identifier",
        "com.apple.developer.team-identifier",
        "com.apple.developer.networking.networkextension",
        "com.apple.security.application-groups",
        "keychain-access-groups",
        "com.apple.private.security.no-container",
        "com.apple.private.persona-mgmt",
    ]
    for k in keys {
        // SecTaskCopyValueForEntitlement 的 Swift 导入返回 CFTypeRef?
        //（ARC 管理，非 Unmanaged），error 参数是 CFError? 而非
        // Unmanaged<CFError>?——三处签名务必对齐 SDK，否则编译失败
        var err: CFError? = nil
        if let v = SecTaskCopyValueForEntitlement(task, k as CFString, &err) {
            out[k] = v
        }
    }
    return out
}

// MARK: - MCM 容器构造（与 TrollStore RootHelper/uicache.m 同源）

typealias ContainerFn = @convention(c) (
    AnyObject, Selector, NSString, Bool,
    UnsafeMutablePointer<Bool>?, UnsafeMutablePointer<CFError?>?
) -> CFTypeRef?

func mcmContainerURL(className: String, id: String) -> String? {
    guard let cls = NSClassFromString(className) else { return nil }
    let obj = cls as AnyObject
    let sel = NSSelectorFromString(
        "containerWithIdentifier:createIfNecessary:existed:error:")
    guard obj.responds(to: sel),
          let sym = dlsym(dlopen(nil, RTLD_LAZY), "objc_msgSend") else { return nil }
    let fn = unsafeBitCast(sym, to: ContainerFn.self)
    guard let c = fn(obj, sel, id as NSString, true, nil, nil) else { return nil }
    let urlSel = NSSelectorFromString("url")
    guard let u = unsafeBitCast(c, to: NSObject.self)
        .perform(urlSel)?.takeUnretainedValue() as? URL else { return nil }
    return u.path
}

// MARK: - 注册（register 子命令）

func loadPlist(_ path: String) -> [String: Any]? {
    (NSDictionary(contentsOfFile: path) as? [String: Any])
}

func register(mainPath: String, appexPath: String) {
    guard
        let mainInfo = loadPlist(mainPath + "/Info.plist"),
        let appexInfo = loadPlist(appexPath + "/Info.plist"),
        let mainBid = mainInfo["CFBundleIdentifier"] as? String,
        let appexBid = appexInfo["CFBundleIdentifier"] as? String,
        let mainExec = mainInfo["CFBundleExecutable"] as? String,
        let appexExec = appexInfo["CFBundleExecutable"] as? String
    else {
        print("result=reg err=readinfo")
        exit(3)
    }

    let mainEnt = dumpEntitlements(from: mainPath + "/" + mainExec)
    let appexEnt = dumpEntitlements(from: appexPath + "/" + appexExec)

    // App Group 容器（createIfNecessary，root 身份创建更稳）
    var groupContainers = [String: String]()
    let groups = mainEnt["com.apple.security.application-groups"] as? [String] ?? []
    for g in groups {
        if let p = mcmContainerURL(
            className: "MCMSharedDataContainer", id: g) {
            groupContainers[g] = p
        }
    }

    // plugin 数据容器
    let pluginContainer = mcmContainerURL(
        className: "MCMPluginKitPluginDataContainer", id: appexBid)

    // ---- plugin 注册字典（逐字段对齐 RootHelper/uicache.m）----
    var plugin: [String: Any] = [
        "ApplicationType": "PluginKitPlugin",
        "CFBundleIdentifier": appexBid,
        "CodeInfoIdentifier": appexBid,
        "CompatibilityState": 0,
        "IsContainerized": true,
        "Path": appexPath,
        "PluginOwnerBundleID": mainBid,
        "SignerOrganization": "Apple Inc.",
        "SignatureVersion": 132352,
        "SignerIdentity": "Apple iPhone OS Application Signing",
        "Entitlements": appexEnt,
    ]
    if let pc = pluginContainer {
        plugin["Container"] = pc
        plugin["EnvironmentVariables"] = [
            "CFFIXED_USER_HOME": pc,
            "HOME": pc,
            "TMPDIR": (pc as NSString).appendingPathComponent("tmp"),
        ]
    }
    if let team = appexEnt["com.apple.developer.team-identifier"] as? String {
        plugin["TeamIdentifier"] = team
    }
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
        "Container": mainPath,
        "IsDeletable": true,
        "Path": mainPath,
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
        "EnvironmentVariables": [
            "CFFIXED_USER_HOME": mainPath,
            "HOME": mainPath,
            "TMPDIR": (mainPath as NSString).appendingPathComponent("tmp"),
        ],
        "_LSBundlePlugins": [appexBid: plugin],
    ]
    if let team = mainEnt["com.apple.developer.team-identifier"] as? String {
        dict["TeamIdentifier"] = team
    }
    if !groupContainers.isEmpty {
        dict["HasAppGroupContainers"] = true
        dict["GroupContainers"] = groupContainers
    }

    // ---- LSApplicationWorkspace（root 身份）----
    guard
        let wsCls = NSClassFromString("LSApplicationWorkspace"),
        let ws = (wsCls as NSObject.Type)
            .perform(NSSelectorFromString("defaultWorkspace"))?
            .takeUnretainedValue()
    else {
        print("result=reg err=no_workspace")
        exit(4)
    }

    let regSel = NSSelectorFromString("registerApplicationDictionary:")
    let lsOk = (ws.perform(regSel, with: dict)?
        .takeUnretainedValue() as? Bool) ?? false

    // registerPlugin: 直接注册插件（参数形态按 iOS 版本二选一）
    let rpSel = NSSelectorFromString("registerPlugin:")
    var pluginOk = false
    for arg in [appexPath, URL(fileURLWithPath: appexPath)] {
        if (ws.perform(rpSel, with: arg)?
            .takeUnretainedValue() as? Bool) == true {
            pluginOk = true
            break
        }
    }

    // 注册后强制 SpringBoard 重载图标缓存（TrollStore uicache 同款）
    var sbOk = false
    let rsSel = NSSelectorFromString("reloadData")
    if ws.responds(to: rsSel) {
        ws.perform(rsSel)
        sbOk = true
    }

    // pkd/nesessionmanager 踢活（root 可发信号；失败不影响主流程）
    func kick(_ label: String, service: String) -> Int32 {
        var argv: [UnsafeMutablePointer<CChar>?] = [
            strdup("/bin/launchctl"), strdup("kickstart"), strdup("-k"),
            strdup("system/\(service)"), nil,
        ]
        defer { argv.prefix(while: { $0 != nil }).forEach { free($0) } }
        var p: pid_t = 0
        let r = posix_spawn(&p, "/bin/launchctl", nil, nil, &argv, nil)
        if r != 0 { return -1 }
        var st: Int32 = 0
        waitpid(p, &st, 0)
        return (st >> 8) & 0xFF
    }
    let pkdKick = kick("pkd", service: "com.apple.pluginkit.pkd")
    let nesmKick = kick("nesm", service: "com.apple.nesessionmanager")

    print("result=reg ls=\(lsOk ? 1 : 0) plugin=\(pluginOk ? 1 : 0) reload=\(sbOk ? 1 : 0) pkdKick=\(pkdKick) nesmKick=\(nesmKick)")
    print("result=bids main=\(mainBid) appex=\(appexBid)")
    print("result=ent mainKeys=\(mainEnt.keys.sorted().count) appexKeys=\(appexEnt.keys.sorted().count)")
    print("result=containers groups=\(groupContainers.keys.sorted().joined(separator: ",")) plugin=\(pluginContainer ?? "nil")")
}

// MARK: - 权限修复（fixperm 子命令）

func isMachO(_ path: String) -> Bool {
    guard let fh = FileHandle(forReadingAtPath: path) else { return false }
    defer { try? fh.close() }
    let magic4 = fh.readData(ofLength: 4)
    guard magic4.count == 4 else { return false }
    let m = [UInt8](magic4)
    // MH_MAGIC_64 / MH_MAGIC（host/little endian）+ FAT
    if m[0] == 0xcf && m[1] == 0xfa && m[2] == 0xed && m[3] == 0xfe { return true }
    if m[3] == 0xcf && m[2] == 0xfa && m[1] == 0xed && m[0] == 0xfe { return true }
    if m[0] == 0xca && m[1] == 0xfe && m[2] == 0xba && m[3] == 0xbe { return true }
    return false
}

func fixperm(mainPath: String, appexPath: String) {
    var fixed = 0
    var failed = 0
    func fix(_ root: String) {
        chown(root, MOBILE_UID, MOBILE_GID)
        chmod(root, 0o755)
        fixed += 1
        let fm = FileManager.default
        guard let subs = try? fm.contentsOfDirectory(atPath: root) else { return }
        for s in subs {
            let p = (root as NSString).appendingPathComponent(s)
            var st = stat()
            if lstat(p, &st) != 0 { failed += 1; continue }
            chown(p, MOBILE_UID, MOBILE_GID)
            if (st.st_mode & S_IFMT) == S_IFDIR {
                chmod(p, 0o755)
                fixed += 1
                fix(p)
            } else {
                // Mach-O 可执行保持 0755，其余 0644
                chmod(p, isMachO(p) ? 0o755 : 0o644)
                fixed += 1
            }
        }
    }
    fix(mainPath)
    fix(appexPath)
    // 注册数据库目录属主修正（pkd 数据库在 mobile 家目录）
    let pkdDir = "/var/mobile/Library/PlugInKit"
    if FileManager.default.fileExists(atPath: pkdDir) {
        chown(pkdDir, MOBILE_UID, MOBILE_GID)
        chmod(pkdDir, 0o755)
        print("result=fixperm pkdDir=fixed")
    } else {
        print("result=fixperm pkdDir=absent")
    }
    print("result=fixperm fixed=\(fixed) failed=\(failed)")
}

// MARK: - 入口

let args = CommandLine.arguments
guard args.count >= 4 else {
    print("result=usage register|fixperm <mainAppPath> <appexPath>")
    exit(2)
}
switch args[1] {
case "register": register(mainPath: args[2], appexPath: args[3])
case "fixperm": fixperm(mainPath: args[2], appexPath: args[3])
default:
    print("result=unknown cmd=\(args[1])")
    exit(2)
}
