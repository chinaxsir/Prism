// Prism 主 App 侧隧道控制：Rust 经 C ABI（@_cdecl）调用，
// 通过 NETunnelProviderManager 启动/停止 PrismVPN 网络扩展。
// 内核（Go libbox）与 TUN 全部运行在扩展进程内。

import Foundation
import NetworkExtension

private let extensionBundleIdentifier = "com.prism.proxy.PrismVPN"
private let managerServerTag = "Prism"

// MARK: - C ABI 入口（供 Rust 调用）

@_cdecl("prism_ios_vpn_start")
func prismIosVpnStart(_ config: UnsafePointer<CChar>) -> Int32 {
    let configText = String(cString: config)
    let semaphore = DispatchSemaphore(value: 0)
    var result: Int32 = 0

    PrismTunnelController.shared.start(config: configText) { error in
        if error != nil {
            result = -1
        }
        semaphore.signal()
    }
    semaphore.wait()
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
                        completion(saveError)
                        return
                    }
                    // save 后重新加载，使 connection 指向持久化后的配置
                    self.reload(manager: manager) { reloadResult in
                        switch reloadResult {
                        case .failure(let error):
                            completion(error)
                        case .success(let saved):
                            do {
                                try saved.connection.startVPNTunnel()
                                completion(nil)
                            } catch {
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
                completion(.failure(error))
                return
            }
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
