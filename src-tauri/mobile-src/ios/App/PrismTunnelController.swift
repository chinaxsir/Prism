// Prism 主 App 侧隧道控制：Rust 经 C ABI（@_cdecl）调用，
// 通过 NETunnelProviderManager 启动/停止 PrismVPN 网络扩展。
// 内核（Go libbox）与 TUN 全部运行在扩展进程内。

import Foundation
import NetworkExtension

/// 扩展 Bundle ID = 主 App Bundle ID + ".PrismVPN"
/// 不写死，避免 tauri ios init 追加后缀导致的不匹配
private let extensionBundleIdentifier: String = {
    let main = Bundle.main.bundleIdentifier ?? "com.prism.proxy"
    return "\(main).PrismVPN"
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
        let path = (NSTemporaryDirectory() as NSString)
            .appendingPathComponent("prism_vpn_start_error.txt")
        NSLog("[PrismVPN] 写错误详情到 \(path): \(errorDesc)")
        try? errorDesc.write(
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

    /// 把 NSError 完整描述（domain/code/userInfo）输出，便于真机定位
    static func describe(error: Error) -> String {
        let ns = error as NSError
        let userInfo = ns.userInfo.map { (k, v) -> String in
            "\(k)=\(v)"
        }.joined(separator: "; ")
        return "domain=\(ns.domain) code=\(ns.code) desc=\(ns.localizedDescription) userInfo={\(userInfo)}"
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

        loadOrCreateManager { result in
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
            // save 后重新加载，使 connection 指向持久化后的配置
            self.reload(manager: manager) { reloadResult in
                switch reloadResult {
                case .failure(let error):
                    NSLog("[PrismVPN] reload 失败: \(PrismTunnelController.describe(error: error))")
                    self.recoverOrFinish(
                        error, configPath: configPath,
                        attemptsRemaining: attemptsRemaining, completion: completion
                    )
                case .success(let saved):
                    do {
                        try saved.connection.startVPNTunnel()
                        NSLog("[PrismVPN] startVPNTunnel 调用成功")

                        // 等待 NEVPNStatus.connected，最多 20s
                        // startVPNTunnel 仅发起启动请求；扩展需异步拉起
                        // libbox 内核、setTunnelNetworkSettings、注册路由，
                        // 全部完成才进入 connected。主 App 立即返回 Running
                        // 会导致前端 clash API 调用失败（节点页空白）。
                        self.waitForConnected(connection: saved.connection, timeout: 20) { err in
                            if let err = err {
                                self.recoverOrFinish(
                                    err, configPath: configPath,
                                    attemptsRemaining: attemptsRemaining, completion: completion
                                )
                            } else {
                                completion(nil)
                            }
                        }
                    } catch {
                        NSLog("[PrismVPN] startVPNTunnel 失败: \(PrismTunnelController.describe(error: error))")
                        self.recoverOrFinish(
                            error, configPath: configPath,
                            attemptsRemaining: attemptsRemaining, completion: completion
                        )
                    }
                }
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
