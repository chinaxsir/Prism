// Prism iOS 网络扩展：NEPacketTunnelProvider。
//
// 语义关键点（build 138 修正）：Go 内核封装的 PrismVPNStart 并非
// "阻塞到退出才返回"的函数——它初始化 sing-box、同步回调 openTun
// （拿到 TUN fd）、拉起后台协程后立即返回：nil = 启动成功，
// 非 nil = 错误字符串。此前版本误把它当阻塞函数，返回后即当作
// "内核退出"处理，造成短暂显示已连接后 ~10 秒报 code=12。
// 生命周期因此大幅简化：
//   startTunnel → 后台串行队列执行 PrismVPNStart（openTun 同步回调
//   内完成 setTunnelNetworkSettings + fd 获取）→ 返回 nil → running +
//   completionHandler(nil)
//   stopTunnel → 同一队列串行执行 PrismVPNStop（Go Close 同步回收）
//   → stopped → completionHandler

import Foundation
import NetworkExtension

private let prismAppGroupID = "group.com.prism.proxy"
private var prismProviderRef: PacketTunnelProvider?

private func writeSharedError(_ message: String) {
    guard let groupURL = FileManager.default.containerURL(
        forSecurityApplicationGroupIdentifier: prismAppGroupID
    ) else { return }
    let url = groupURL.appendingPathComponent("prism_error.txt")
    try? message.write(to: url, atomically: true, encoding: .utf8)
}

private func clearSharedError() {
    guard let groupURL = FileManager.default.containerURL(
        forSecurityApplicationGroupIdentifier: prismAppGroupID
    ) else { return }
    try? FileManager.default.removeItem(
        at: groupURL.appendingPathComponent("prism_error.txt")
    )
}

private func writeHeartbeat(_ stage: String) {
    guard let groupURL = FileManager.default.containerURL(
        forSecurityApplicationGroupIdentifier: prismAppGroupID
    ) else { return }
    let ts = Int(Date().timeIntervalSince1970)
    try? "\(ts) \(stage)".write(
        to: groupURL.appendingPathComponent("prism_status.txt"),
        atomically: true, encoding: .utf8
    )
}

final class PacketTunnelProvider: NEPacketTunnelProvider {

    private enum TunnelState {
        case stopped
        case starting
        case running
        case stopping
    }

    /// 串行队列：保证 start 与 stop 严格排队，且不阻塞主线程 RunLoop
    /// （openTun 里 setTunnelNetworkSettings 的系统回调依赖主 RunLoop
    /// 空闲，若在主线程同步等信号量会死锁 10 秒超时）
    private let vpnQueue = DispatchQueue(label: "com.prism.proxy.vpnQueue", qos: .userInitiated)
    private var currentState: TunnelState = .stopped
    private let stateLock = NSLock()
    private var heartbeatTimer: Timer?

    override func startTunnel(
        options: [String: NSObject]?,
        completionHandler: @escaping (Error?) -> Void
    ) {
        prismProviderRef = self
        NSLog("[PrismVPN] startTunnel begin")
        writeHeartbeat("start")
        clearSharedError()

        stateLock.lock()
        if currentState != .stopped {
            stateLock.unlock()
            let err = NSError(domain: "PrismVPN", code: -1, userInfo: [NSLocalizedDescriptionKey: "内核正在运行或状态切换中"])
            completionHandler(err)
            return
        }
        currentState = .starting
        stateLock.unlock()

        let configText: String
        if let configPath = (protocolConfiguration as? NETunnelProviderProtocol)?
            .providerConfiguration?["config_path"] as? String,
           !configPath.isEmpty
        {
            do {
                configText = try String(contentsOfFile: configPath, encoding: .utf8)
            } catch {
                let msg = "无法读取共享配置文件: \(error.localizedDescription)"
                failStart(code: 3, message: msg, completionHandler: completionHandler)
                return
            }
        } else if let inlineConfig = (protocolConfiguration as? NETunnelProviderProtocol)?
            .providerConfiguration?["config"] as? String
        {
            configText = inlineConfig
        } else {
            failStart(code: 1, message: "缺少配置（providerConfiguration.config_path）", completionHandler: completionHandler)
            return
        }

        let workDir = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]

        // 在后台串行队列启动内核，释放主线程以接收 setTunnelNetworkSettings 的系统回调
        vpnQueue.async { [weak self] in
            guard let self = self else { return }

            do {
                try FileManager.default.createDirectory(at: workDir, withIntermediateDirectories: true)
                try self.copyGeoDatabases(to: workDir)

                let callbacks = prism_vpn_cbs(
                    open_tun: prismOpenTun,
                    protect_socket: prismProtectSocket,
                    under_extension: prismUnderExtension
                )
                PrismVPNSetCallbacks(callbacks)

                // PrismVPNStart 内部会同步回调 openTun，启动后台协程后立即返回
                //（nil = 成功，非 nil = 错误字符串）
                let errorPtr = configText.withCString { configPtr in
                    workDir.path.withCString { dirPtr in
                        PrismVPNStart(
                            UnsafeMutablePointer(mutating: configPtr),
                            UnsafeMutablePointer(mutating: dirPtr)
                        )
                    }
                }

                if let errorPtr = errorPtr {
                    let message = String(cString: errorPtr)
                    PrismKernelFree(errorPtr)
                    throw NSError(domain: "PrismVPN", code: 2, userInfo: [NSLocalizedDescriptionKey: message])
                }

                // 启动成功返回后复查状态：期间若已收到 stopTunnel（快速开关），
                // vpnQueue 串行保证排队的 PrismVPNStop 必在其后执行，此时不能
                // 再向系统报"启动成功"，否则 NESM 会在已停止的会话上苦等超时
                self.stateLock.lock()
                let stopping = (self.currentState == .stopping)
                if !stopping { self.currentState = .running }
                self.stateLock.unlock()
                if stopping {
                    NSLog("[PrismVPN] start finished after stop requested; skip completion(nil)")
                    return
                }

                NSLog("[PrismVPN] PrismVPNStart succeeded, tunnel ready")
                writeHeartbeat("running")
                DispatchQueue.main.async {
                    self.startHeartbeatTimer()
                    completionHandler(nil)
                }
            } catch {
                self.failStart(code: 2, message: error.localizedDescription, completionHandler: completionHandler)
            }
        }
    }

    private func failStart(code: Int, message: String, completionHandler: @escaping (Error?) -> Void) {
        NSLog("[PrismVPN] startTunnel failed: \(message)")
        stateLock.lock()
        currentState = .stopped
        stateLock.unlock()
        writeSharedError(message)
        writeHeartbeat("startFailed")
        let err = NSError(domain: "PrismVPN", code: code, userInfo: [NSLocalizedDescriptionKey: message])
        DispatchQueue.main.async {
            completionHandler(err)
        }
    }

    private func startHeartbeatTimer() {
        heartbeatTimer?.invalidate()
        let t = Timer.scheduledTimer(withTimeInterval: 3.0, repeats: true) { _ in
            writeHeartbeat("alive")
        }
        RunLoop.main.add(t, forMode: .common)
        heartbeatTimer = t
    }

    override func stopTunnel(
        with reason: NEProviderStopReason,
        completionHandler: @escaping () -> Void
    ) {
        stateLock.lock()
        currentState = .stopping
        stateLock.unlock()

        DispatchQueue.main.async {
            self.heartbeatTimer?.invalidate()
            self.heartbeatTimer = nil
        }

        vpnQueue.async { [weak self] in
            PrismVPNStop()
            writeHeartbeat("stopped:\(reason.rawValue)")
            if let self = self {
                self.stateLock.lock()
                self.currentState = .stopped
                self.stateLock.unlock()
            }
            DispatchQueue.main.async {
                completionHandler()
            }
        }
    }

    fileprivate func openTun(optionsJSON: String) -> Int32 {
        NSLog("[PrismVPN] openTun called with options: \(optionsJSON)")
        writeHeartbeat("openTun")
        guard let data = optionsJSON.data(using: .utf8),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            writeSharedError("openTun: JSON 解析失败")
            return -1
        }

        let mtu = (json["mtu"] as? NSNumber)?.int32Value ?? 9000
        let dnsServer = json["dns_server"] as? String ?? ""

        let settings = NEPacketTunnelNetworkSettings(tunnelRemoteAddress: "127.0.0.1")
        settings.mtu = NSNumber(value: mtu)

        if !dnsServer.isEmpty {
            settings.dnsSettings = NEDNSSettings(servers: [dnsServer])
        }

        let v4Addresses = prefixes(json["inet4_address"])
        if !v4Addresses.isEmpty {
            let ipv4 = NEIPv4Settings(
                addresses: v4Addresses.map(\.address),
                subnetMasks: v4Addresses.map { subnetMask(prefix: $0.prefix) }
            )
            let v4Routes = prefixes(json["inet4_route_address"])
            if v4Routes.isEmpty {
                ipv4.includedRoutes = [NEIPv4Route.default()]
            } else {
                ipv4.includedRoutes = v4Routes.map {
                    NEIPv4Route(destinationAddress: $0.address, subnetMask: subnetMask(prefix: $0.prefix))
                }
            }
            settings.ipv4Settings = ipv4
        }

        let v6Addresses = prefixes(json["inet6_address"])
        if !v6Addresses.isEmpty {
            let ipv6 = NEIPv6Settings(
                addresses: v6Addresses.map(\.address),
                networkPrefixLengths: v6Addresses.map { NSNumber(value: $0.prefix) }
            )
            let v6Routes = prefixes(json["inet6_route_address"])
            if v6Routes.isEmpty {
                ipv6.includedRoutes = [NEIPv6Route.default()]
            } else {
                ipv6.includedRoutes = v6Routes.map {
                    NEIPv6Route(destinationAddress: $0.address, networkPrefixLength: NSNumber(value: $0.prefix))
                }
            }
            settings.ipv6Settings = ipv6
        }

        let semaphore = DispatchSemaphore(value: 0)
        var settingsError: Error?
        setTunnelNetworkSettings(settings) { error in
            settingsError = error
            semaphore.signal()
        }

        let waitResult = semaphore.wait(timeout: .now() + 10)
        if waitResult == .timedOut {
            writeSharedError("openTun: setTunnelNetworkSettings 10s 超时")
            writeHeartbeat("tunTimeout")
            return -1
        }

        if let settingsError = settingsError {
            writeSharedError("openTun: setTunnelNetworkSettings 失败: \(settingsError.localizedDescription)")
            return -1
        }
        writeHeartbeat("tun-ok")

        let fd: Int32? = (
            packetFlow.value(forKeyPath: "socket.fileDescriptor") as? Int32
        ) ?? (
            packetFlow.value(forKeyPath: "socket.socketDescriptor") as? Int32
        )
        guard let fd = fd, fd >= 0 else {
            writeSharedError("openTun: 无法获取 packetFlow fileDescriptor")
            writeHeartbeat("tunFdFail")
            return -1
        }
        writeHeartbeat("tun-fd")
        return fd
    }

    private func copyGeoDatabases(to workDir: URL) throws {
        for name in ["geoip.db", "geosite.db"] {
            guard let src = Bundle.main.url(forResource: name, withExtension: nil) else {
                throw NSError(
                    domain: "PrismVPN", code: 3,
                    userInfo: [NSLocalizedDescriptionKey: "扩展包内缺少 \(name)"]
                )
            }
            let dst = workDir.appendingPathComponent(name)
            if FileManager.default.fileExists(atPath: dst.path) {
                continue
            }
            try FileManager.default.copyItem(at: src, to: dst)
        }
    }
}

private struct Prefix {
    let address: String
    let prefix: Int
}

private func prefixes(_ raw: Any?) -> [Prefix] {
    guard let array = raw as? [[String: Any]] else { return [] }
    return array.compactMap { item in
        guard let address = item["address"] as? String,
              let prefix = item["prefix"] as? Int
        else { return nil }
        return Prefix(address: address, prefix: prefix)
    }
}

private func subnetMask(prefix: Int) -> String {
    var mask = [UInt8](repeating: 0, count: 4)
    let bits = min(max(prefix, 0), 32)
    for i in 0..<bits {
        mask[i / 8] |= 0x80 >> (i % 8)
    }
    return mask.map { String($0) }.joined(separator: ".")
}

@_cdecl("prism_open_tun")
private func prismOpenTun(_ json: UnsafePointer<CChar>?) -> Int32 {
    guard let json = json, let provider = prismProviderRef else { return -1 }
    return provider.openTun(optionsJSON: String(cString: json))
}

@_cdecl("prism_protect_socket")
private func prismProtectSocket(_ fd: Int32) -> Int32 {
    return 0
}

@_cdecl("prism_under_extension")
private func prismUnderExtension() -> Int32 {
    return 1
}
