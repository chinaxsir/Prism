// Prism iOS 网络扩展：NEPacketTunnelProvider。
// 参考 sing-box 官方 SFI Extension（PacketTunnelProvider / PlatformInterface）。
//
// 启动流程（主 App 经 NETunnelProviderManager 发起）：
//   主 App 把配置 JSON 写到 App Group 共享容器 prism_config.json，
//   providerConfiguration 只传 "config_path"（路径很短，避开 512KB 限制）；
//   geoip/geosite 从扩展 Bundle 拷入工作目录；
//   注册 Go libbox 回调后 PrismVPNStart → OpenTun → setTunnelNetworkSettings + TUN fd。

import Foundation
import NetworkExtension

/// App Group 共享容器 ID（主 App 与扩展共用，TrollStore 可签任意 group）
private let prismAppGroupID = "group.com.prism.proxy"

// Go 线程回调时需要访问当前扩展实例
private var prismProviderRef: PacketTunnelProvider?

// MARK: - 共享错误文件（与主 App 的断开原因传递通道）
//
// NEVPNConnection / NETunnelProviderSession 在工程部署目标（iOS 14）上
// 没有任何同步属性能拿到 startTunnel completionHandler 返回的错误；
// iOS 16+ 的 fetchLastDisconnectError 还是异步且只给笼统的内部错误。
// 因此扩展在失败时把精确错误写入 App Group「prism_error.txt」，
// 主 App 在 status=disconnected 时直接读取——版本无关、信息无损。

private func writeSharedError(_ message: String) {
    guard let groupURL = FileManager.default.containerURL(
        forSecurityApplicationGroupIdentifier: prismAppGroupID
    ) else {
        NSLog("[PrismVPN] App Group 不可用，无法写 prism_error.txt")
        return
    }
    let url = groupURL.appendingPathComponent("prism_error.txt")
    do {
        try message.write(to: url, atomically: true, encoding: .utf8)
        NSLog("[PrismVPN] 已写共享错误: \(message)")
    } catch {
        NSLog("[PrismVPN] 写 prism_error.txt 失败: \(error.localizedDescription)")
    }
}

private func clearSharedError() {
    guard let groupURL = FileManager.default.containerURL(
        forSecurityApplicationGroupIdentifier: prismAppGroupID
    ) else { return }
    try? FileManager.default.removeItem(
        at: groupURL.appendingPathComponent("prism_error.txt")
    )
}

final class PacketTunnelProvider: NEPacketTunnelProvider {

    override func startTunnel(
        options: [String: NSObject]?,
        completionHandler: @escaping (Error?) -> Void
    ) {
        prismProviderRef = self
        NSLog("[PrismVPN] startTunnel begin")
        // 清理上一轮残留错误，避免本次扩展进程崩溃（来不及写新错误）时
        // 主 App 读到陈旧内容被误导
        clearSharedError()

        // 优先从 providerConfiguration["config_path"] 读路径，再读文件
        // （避免把完整配置塞进 providerConfiguration 触发 512KB 限制）
        let configText: String
        if let configPath = (protocolConfiguration as? NETunnelProviderProtocol)?
            .providerConfiguration?["config_path"] as? String,
           !configPath.isEmpty
        {
            NSLog("[PrismVPN] reading config from path: \(configPath)")
            do {
                configText = try String(contentsOfFile: configPath, encoding: .utf8)
                NSLog("[PrismVPN] config loaded, length=\(configText.count)")
            } catch {
                NSLog("[PrismVPN] failed to read config: \(error.localizedDescription)")
                let msg = "无法读取共享配置文件: \(error.localizedDescription)"
                writeSharedError(msg)
                completionHandler(NSError(
                    domain: "PrismVPN", code: 3,
                    userInfo: [NSLocalizedDescriptionKey: msg]
                ))
                return
            }
        } else if let inlineConfig = (protocolConfiguration as? NETunnelProviderProtocol)?
            .providerConfiguration?["config"] as? String
        {
            // 兼容旧路径（小配置场景）：直接内嵌
            NSLog("[PrismVPN] using inline config, length=\(inlineConfig.count)")
            configText = inlineConfig
        } else {
            NSLog("[PrismVPN] missing config_path in providerConfiguration")
            let msg = "缺少配置（providerConfiguration.config_path）"
            writeSharedError(msg)
            completionHandler(NSError(
                domain: "PrismVPN", code: 1,
                userInfo: [NSLocalizedDescriptionKey: msg]
            ))
            return
        }

        let workDir = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        NSLog("[PrismVPN] workDir=\(workDir.path)")
        do {
            try FileManager.default.createDirectory(at: workDir, withIntermediateDirectories: true)
            try copyGeoDatabases(to: workDir)
            NSLog("[PrismVPN] geo databases ready")

            let callbacks = prism_vpn_cbs(
                open_tun: prismOpenTun,
                protect_socket: prismProtectSocket,
                under_extension: prismUnderExtension
            )
            PrismVPNSetCallbacks(callbacks)
            NSLog("[PrismVPN] callbacks set, calling PrismVPNStart")

            // Go 导出签名是 char*（Swift 侧为 UnsafeMutablePointer<CChar>），
            // String 只能隐式桥接到 UnsafePointer，需显式 mutating 转换；
            // Go 侧在调用期间立即 C.GoString 拷贝，指针仅在调用期内有效
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
                NSLog("[PrismVPN] PrismVPNStart failed: \(message)")
                throw NSError(
                    domain: "PrismVPN", code: 2,
                    userInfo: [NSLocalizedDescriptionKey: message]
                )
            }

            NSLog("[PrismVPN] PrismVPNStart succeeded, tunnel ready")
            completionHandler(nil)
        } catch {
            NSLog("[PrismVPN] startTunnel error: \(error.localizedDescription)")
            // 含 PrismVPNStart 返回的精确内核错误（Go error 原文）
            writeSharedError(error.localizedDescription)
            completionHandler(error)
        }
    }

    override func stopTunnel(
        with reason: NEProviderStopReason,
        completionHandler: @escaping () -> Void
    ) {
        PrismVPNStop()
        completionHandler()
    }

    // MARK: - OpenTun（Go libbox 回调进入，在 Go 线程执行）

    fileprivate func openTun(optionsJSON: String) -> Int32 {
        NSLog("[PrismVPN] openTun called with options: \(optionsJSON)")
        guard let data = optionsJSON.data(using: .utf8),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            NSLog("[PrismVPN] openTun: failed to parse options JSON")
            return -1
        }

        let mtu = (json["mtu"] as? NSNumber)?.int32Value ?? 9000
        let dnsServer = json["dns_server"] as? String ?? ""
        NSLog("[PrismVPN] openTun: mtu=\(mtu) dns=\(dnsServer)")

        let settings = NEPacketTunnelNetworkSettings(tunnelRemoteAddress: "127.0.0.1")
        settings.mtu = NSNumber(value: mtu)

        if !dnsServer.isEmpty {
            settings.dnsSettings = NEDNSSettings(servers: [dnsServer])
        }

        // IPv4：地址 + includedRoutes
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

        // IPv6（可选）
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
        semaphore.wait()

        if let settingsError = settingsError {
            NSLog("[PrismVPN] openTun: setTunnelNetworkSettings failed: \(settingsError.localizedDescription)")
            return -1
        }
        NSLog("[PrismVPN] openTun: setTunnelNetworkSettings succeeded")

        // 取 packetFlow 文件描述符（与 SFI 相同的 KVC 路径）。
        // iOS 不同版本 KVC 键可能不同，依次尝试常见路径。
        let fd: Int32? = (
            packetFlow.value(forKeyPath: "socket.fileDescriptor") as? Int32
        ) ?? (
            packetFlow.value(forKeyPath: "socket.socketDescriptor") as? Int32
        )
        guard let fd = fd, fd >= 0 else {
            NSLog("[PrismVPN] openTun: failed to get packetFlow fileDescriptor (tried socket.fileDescriptor / socket.socketDescriptor)")
            return -1
        }
        NSLog("[PrismVPN] openTun: got packetFlow fd=\(fd)")
        return fd
    }

    // MARK: - Geo 数据库

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

// MARK: - 地址前缀解析

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

// MARK: - Go C 回调（@_cdecl）

@_cdecl("prism_open_tun")
private func prismOpenTun(_ json: UnsafePointer<CChar>?) -> Int32 {
    guard let json = json, let provider = prismProviderRef else { return -1 }
    return provider.openTun(optionsJSON: String(cString: json))
}

@_cdecl("prism_protect_socket")
private func prismProtectSocket(_ fd: Int32) -> Int32 {
    // iOS 网络扩展的底层接口选择由 UsePlatformAutoDetectInterfaceControl
    // 之外的系统机制处理；protect 在 iOS 为 no-op（与 SFI 一致）
    return 0
}

@_cdecl("prism_under_extension")
private func prismUnderExtension() -> Int32 {
    return 1
}
