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

    func start(configPath: String, completion: @escaping (Error?) -> Void) {
        loadOrCreateManager { result in
            switch result {
            case .failure(let error):
                completion(error)
            case .success(let manager):
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
                        completion(saveError)
                        return
                    }
                    NSLog("[PrismVPN] saveToPreferences 成功, extBundleId=\(extensionBundleIdentifier)")
                    // save 后重新加载，使 connection 指向持久化后的配置
                    self.reload(manager: manager) { reloadResult in
                        switch reloadResult {
                        case .failure(let error):
                            NSLog("[PrismVPN] reload 失败: \(PrismTunnelController.describe(error: error))")
                            completion(error)
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
                                    completion(err)
                                }
                            } catch {
                                NSLog("[PrismVPN] startVPNTunnel 失败: \(PrismTunnelController.describe(error: error))")
                                completion(error)
                            }
                        }
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
                self.describeDisconnect(connection: connection) { desc in
                    finish(NSError(
                        domain: "PrismVPN", code: -2,
                        userInfo: [NSLocalizedDescriptionKey: desc]
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
    ///    来不及写文件的场景）
    /// 3. 通用描述
    private func describeDisconnect(
        connection: NEVPNConnection,
        completion: @escaping (String) -> Void
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
                completion(text)
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
                DispatchQueue.main.async { completion(desc) }
            }
        } else {
            // 3. iOS 14/15 无系统 API 且扩展未写文件（如进程被系统直接杀死）
            completion(generic)
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
            NSLog("[PrismVPN] loadAllFromPreferences 成功, 找到 \(managers?.count ?? 0) 个 manager")
            let existing = managers?.first {
                ($0.protocolConfiguration as? NETunnelProviderProtocol)?.serverAddress == managerServerTag
            }
            completion(.success(existing ?? NETunnelProviderManager()))
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
            let saved = managers?.first {
                ($0.protocolConfiguration as? NETunnelProviderProtocol)?.serverAddress == managerServerTag
            }
            completion(.success(saved ?? manager))
        }
    }
}
