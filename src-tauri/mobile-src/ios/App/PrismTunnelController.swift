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

private let managerServerTag = "Prism"

// MARK: - C ABI 入口（供 Rust 调用）

@_cdecl("prism_ios_vpn_start")
func prismIosVpnStart(_ config: UnsafePointer<CChar>) -> Int32 {
    let configText = String(cString: config)
    let semaphore = DispatchSemaphore(value: 0)
    var result: Int32 = 0
    var errorDesc: String = ""

    PrismTunnelController.shared.start(config: configText) { error in
        if let error = error {
            result = -1
            errorDesc = PrismTunnelController.describe(error: error)
        }
        semaphore.signal()
    }
    semaphore.wait()

    if result != 0 {
        // 把详细错误写入临时文件，Rust 侧读出回传前端
        let path = "/tmp/prism_vpn_start_error.txt"
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

    func start(config: String, completion: @escaping (Error?) -> Void) {
        loadOrCreateManager { result in
            switch result {
            case .failure(let error):
                completion(error)
            case .success(let manager):
                let proto = NETunnelProviderProtocol()
                proto.providerBundleIdentifier = extensionBundleIdentifier
                proto.serverAddress = managerServerTag
                proto.disconnectOnSleep = false
                proto.providerConfiguration = ["config": config]

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
                                completion(nil)
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

    func stop(completion: @escaping (Error?) -> Void) {
        loadOrCreateManager { result in
            switch result {
            case .failure(let error):
                completion(error)
            case .success(let manager):
                manager.connection.stopVPNTunnel()
                completion(nil)
            }
        }
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
