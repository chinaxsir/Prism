// Prism iOS 网络扩展：NEPacketTunnelProvider（巨魔高并发防弹版）。
//
// 巨魔（TrollStore）环境下 iOS 对 NetworkExtension 的生命周期管理
// 远比 App Store 粗暴：内存易被 Jetsam 强杀、pkd 进程树容易脱节、
// 主 App 可能收不到状态变化。本版本针对这些痛点做了三层防御：
//
//   1. 状态与回调剥离：startTunnel 的 completionHandler 暂存不发，
//      只有 Go 内核回调 openTun 且 setTunnelNetworkSettings 成功、
//      拿到 TUN fd 后才调 completionHandler(nil) 向系统报喜——
//      彻底消灭"系统已连接、内核却没起来"的假连接。
//
//   2. 内核僵尸防御（coreExitSemaphore 互斥）：同一时刻只允许一个
//      内核生命周期操作（PrismVPNStart / PrismVPNStop）在执行。
//      新的启动必须等上一次操作真正结束，杜绝快速开关导致的
//      双内核并发（Go panic / 端口抢占）。
//
//   3. 安全 Stop：stopTunnel 先等内核启动调用彻底返回，再发
//      PrismVPNStop（Go 侧 Close 同步等待全部资源释放），返回后
//      才向系统回报 Stop 完成——不产生"指令发出但内核未退出"。
//
// 内核真实语义（src-tauri/ios-kernel/vpn.go，sing-box v1.11 libbox）：
//   PrismVPNStart 非阻塞——NewService + Start() 完成后即返回 nil
//   （期间同步触发 openTun 回调）；返回非 nil = 启动失败，服务已被
//   Go 侧回收。内核真正退出只发生在 PrismVPNStop（Close 同步）。
//
// 启动流程（主 App 经 NETunnelProviderManager 发起）：
//   主 App 把配置 JSON 写到 App Group 共享容器 prism_config.json，
//   providerConfiguration 只传 "config_path"（路径很短，避开 512KB 限制）；
//   geoip/geosite 从扩展 Bundle 拷入工作目录；
//   注册 Go libbox 回调后，后台异步执行 PrismVPNStart → 触发 OpenTun
//   回调 → setTunnelNetworkSettings + 返回 TUN fd → 报告启动成功。

import Foundation
import NetworkExtension

/// App Group 共享容器 ID（主 App 与扩展共用，TrollStore 可签任意 group）
private let prismAppGroupID = "group.com.prism.proxy"

// Go 线程回调时需要访问当前扩展实例
private var prismProviderRef: PacketTunnelProvider?

// MARK: - 共享错误文件（与主 App 的断开原因传递通道）

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

// MARK: - 扩展心跳（存活可见化）

/// 每 3s 刷新 prism_status.txt：只要扩展进程活着，时间戳就持续更新；
/// 进程死亡后心跳冻结，主 App 的看门狗据此判死（唯一可靠存活证据）
private func writeHeartbeat(_ stage: String) {
    guard let groupURL = FileManager.default.containerURL(
        forSecurityApplicationGroupIdentifier: prismAppGroupID
    ) else { return }
    let ts = Int(Date().timeIntervalSince1970)
    try? "\(ts) \(stage)".write(
        to: groupURL.appendingPathComponent("prism_status.txt"),
        atomically: true, encoding: .utf8)
}

final class PacketTunnelProvider: NEPacketTunnelProvider {

    private enum TunnelState {
        case stopped
        case starting
        case running
        case stopping
    }

    // MARK: 并发控制与巨魔环境防御锁

    private var currentState: TunnelState = .stopped
    private let stateLock = NSLock()

    /// 内核生命周期互斥锁（1 = 允许操作，0 = 内核操作进行中）。
    /// PrismVPNStart（含 openTun 全程）与 PrismVPNStop 必须互斥：
    /// - 启动未完成时再次启动 → 两个内核实例并发 → Go panic / 端口抢占
    /// - 启动未完成时停止 → 停止指令可能先于服务建立而被 Go 侧
    ///   no-op 掉（vpnService 仍为 nil），造成"停止丢失、内核永生"
    private let coreExitSemaphore = DispatchSemaphore(value: 1)

    /// 暂存系统的启动回调，只有 openTun 成功后才放行（防假连接）
    private var pendingStartCompletion: ((Error?) -> Void)?

    /// 心跳定时器（openTun 成功后启动，stopTunnel / 内核崩溃时停止）
    private var heartbeatTimer: Timer?

    // MARK: - 生命周期：启动

    override func startTunnel(
        options: [String: NSObject]?,
        completionHandler: @escaping (Error?) -> Void
    ) {
        prismProviderRef = self
        clearSharedError()
        writeHeartbeat("start")
        NSLog("[PrismVPN] startTunnel begin")

        // 1. 拦截并发/重复启动（巨魔下系统可能不排队直接重入）
        stateLock.lock()
        if currentState != .stopped {
            stateLock.unlock()
            NSLog("[PrismVPN] 拦截重复启动请求，当前状态: \(currentState)")
            completionHandler(NSError(
                domain: "PrismVPN", code: -1,
                userInfo: [NSLocalizedDescriptionKey: "内核正在运行或切换状态"]
            ))
            return
        }
        currentState = .starting
        pendingStartCompletion = completionHandler // 暂存回调，等待 openTun 唤醒
        stateLock.unlock()

        // 2. 读取配置（config_path 优先；内嵌 config 为旧路径兜底）
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
                notifyStartFailure(error: NSError(
                    domain: "PrismVPN", code: 3,
                    userInfo: [NSLocalizedDescriptionKey:
                        "无法读取共享配置文件: \(error.localizedDescription)"]
                ))
                return
            }
        } else if let inlineConfig = (protocolConfiguration as? NETunnelProviderProtocol)?
            .providerConfiguration?["config"] as? String
        {
            NSLog("[PrismVPN] using inline config, length=\(inlineConfig.count)")
            configText = inlineConfig
        } else {
            NSLog("[PrismVPN] missing config_path in providerConfiguration")
            notifyStartFailure(error: NSError(
                domain: "PrismVPN", code: 1,
                userInfo: [NSLocalizedDescriptionKey:
                    "缺少配置（providerConfiguration.config_path）"]
            ))
            return
        }

        let workDir = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        NSLog("[PrismVPN] workDir=\(workDir.path)")

        // 3. 内核启动派发至后台，绝不阻塞 iOS 扩展的系统回调线程
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self = self else { return }

            // 【关键】必须等待上一次内核生命周期操作彻底结束！
            // 防止巨魔环境下用户快速开关导致的并发崩溃（双内核抢端口）
            self.coreExitSemaphore.wait()
            defer { self.coreExitSemaphore.signal() }

            do {
                try FileManager.default.createDirectory(at: workDir, withIntermediateDirectories: true)
                try self.copyGeoDatabases(to: workDir)
                NSLog("[PrismVPN] geo databases ready")

                let callbacks = prism_vpn_cbs(
                    open_tun: prismOpenTun,
                    protect_socket: prismProtectSocket,
                    under_extension: prismUnderExtension
                )
                PrismVPNSetCallbacks(callbacks)
                NSLog("[PrismVPN] callbacks set, 进入 Go 内核启动调用 PrismVPNStart...")

                // 提前复制值以便在 withCString 闭包中使用
                let finalConfigText = configText
                let finalWorkDirPath = workDir.path

                // PrismVPNStart：启动完成（含 openTun 回调全程）后返回；
                // 失败时返回错误指针（服务已被 Go 侧回收）
                let errorPtr = finalConfigText.withCString { configPtr in
                    finalWorkDirPath.withCString { dirPtr in
                        return PrismVPNStart(
                            UnsafeMutablePointer(mutating: configPtr),
                            UnsafeMutablePointer(mutating: dirPtr)
                        )
                    }
                }

                if let errorPtr = errorPtr {
                    let message = String(cString: errorPtr)
                    PrismKernelFree(errorPtr)
                    NSLog("[PrismVPN] 内核异常退出: \(message)")
                    self.handleCoreExit(withError: NSError(
                        domain: "PrismVPN", code: 2,
                        userInfo: [NSLocalizedDescriptionKey: message]
                    ))
                } else {
                    // 【丢停止防御】启动期间若收到过停止请求，stopTunnel 的
                    // PrismVPNStop 可能先于服务建立执行而被 Go 侧 no-op 掉
                    //（vpnService 仍为 nil）。此处复查状态并补一刀，确保
                    // 刚拉起的服务被立即回收，杜绝"停止完成后内核才运行"
                    // 的僵尸内核（巨魔快速开关的典型事故）
                    self.stateLock.lock()
                    let stopDuringStart = (self.currentState == .stopping
                        || self.currentState == .stopped)
                    self.stateLock.unlock()
                    if stopDuringStart {
                        NSLog("[PrismVPN] 启动期间收到停止请求，补发 PrismVPNStop 回收内核")
                        PrismVPNStop()
                    }
                    NSLog("[PrismVPN] 内核启动调用返回（服务在后台 goroutine 持续运行）")
                    self.handleCoreExit(withError: stopDuringStart
                        ? NSError(
                            domain: "PrismVPN", code: -4,
                            userInfo: [NSLocalizedDescriptionKey: "隧道在启动完成前被停止"]
                        )
                        : nil)
                }
            } catch {
                NSLog("[PrismVPN] startTunnel 后台准备失败: \(error.localizedDescription)")
                self.handleCoreExit(withError: error)
            }
        }
    }

    // MARK: - 生命周期：停止

    override func stopTunnel(
        with reason: NEProviderStopReason,
        completionHandler: @escaping () -> Void
    ) {
        NSLog("[PrismVPN] 收到停止指令，原因=\(reason.rawValue)")

        stateLock.lock()
        if currentState == .stopped || currentState == .stopping {
            stateLock.unlock()
            completionHandler()
            return
        }
        currentState = .stopping
        stateLock.unlock()

        DispatchQueue.main.async {
            self.heartbeatTimer?.invalidate()
            self.heartbeatTimer = nil
        }
        writeHeartbeat("stopping")

        DispatchQueue.global().async {
            // 【关键】巨魔防竞态，两步缺一不可：
            //  a) 先等内核启动调用彻底返回（coreExitSemaphore）——
            //     保证 PrismVPNStop 不会跑在服务建立之前而被 Go 侧
            //     no-op 掉（丢停止、内核永生）；
            //  b) PrismVPNStop 内部 Close 同步等待全部资源释放，
            //     返回即内核彻底退出，此时才向系统回报停止完成。
            self.coreExitSemaphore.wait()
            PrismVPNStop()
            self.coreExitSemaphore.signal()

            self.stateLock.lock()
            self.currentState = .stopped
            self.stateLock.unlock()

            writeHeartbeat("stopped:\(reason.rawValue)")
            DispatchQueue.main.async {
                completionHandler()
            }
        }
    }

    // MARK: - 状态流转处理

    /// openTun 成功（TUN 网卡挂载 + 拿到 fd）后调用：
    /// 只有此刻才向系统报告启动成功，消灭假连接
    private func notifyStartSuccess() {
        stateLock.lock()
        defer { stateLock.unlock() }

        guard currentState == .starting else { return } // stopping 期间不报喜
        currentState = .running
        NSLog("[PrismVPN] openTun 就绪，向系统报告启动成功！")
        writeHeartbeat("running")

        if let completion = pendingStartCompletion {
            pendingStartCompletion = nil
            DispatchQueue.main.async { completion(nil) }
        }
        DispatchQueue.main.async {
            self.startHeartbeatTimer()
        }
    }

    /// 启动失败统一出口：写共享错误 + 冻结心跳阶段 + 放行系统回调
    private func notifyStartFailure(error: Error) {
        stateLock.lock()
        defer { stateLock.unlock() }

        currentState = .stopped
        writeSharedError(error.localizedDescription)
        writeHeartbeat("startFailed")

        if let completion = pendingStartCompletion {
            pendingStartCompletion = nil
            DispatchQueue.main.async { completion(error) }
        }
    }

    /// 内核生命周期操作结束（PrismVPNStart 返回 / 后台准备抛错）。
    /// error == nil：内核启动成功，服务持续运行，无需处理；
    /// error != nil：内核启动失败或运行中崩溃，按当前状态分流。
    private func handleCoreExit(withError error: Error?) {
        // pendingStartCompletion 的消费者只有 openTun 成功路径
        //（notifyStartSuccess，仅在 PrismVPNStart 调用期间触发）与本方法
        //（PrismVPNStart 返回之后），二者时间窗不重叠，可单次读取+清零
        stateLock.lock()
        let wasRunning = (currentState == .running)
        let pending = pendingStartCompletion
        pendingStartCompletion = nil
        stateLock.unlock()

        if let err = error {
            writeSharedError(err.localizedDescription)
        }

        // 启动回调仍被暂存（含启动过程中被停止的场景）：放行系统回调，
        // 避免 iOS 侧 startTunnel 完成回调悬挂导致状态机卡死
        if let completion = pending {
            stateLock.lock()
            currentState = .stopped
            stateLock.unlock()
            writeHeartbeat("startFailed")
            let finalError = error ?? NSError(
                domain: "PrismVPN", code: -3,
                userInfo: [NSLocalizedDescriptionKey: "内核意外退出"]
            )
            DispatchQueue.main.async { completion(finalError) }
            return
        }

        // running 中内核异常退出：主 App 需要被通知断开
        if wasRunning, let err = error {
            writeHeartbeat("crashed")
            NSLog("[PrismVPN] 运行中内核崩溃: \(err.localizedDescription)")
            DispatchQueue.main.async {
                self.heartbeatTimer?.invalidate()
                self.heartbeatTimer = nil
            }
            // 主动通知系统切断 VPN（触发主 App 断开监听）
            self.cancelTunnelWithError(err)
        }
    }

    // MARK: - OpenTun 回调（从 Go 线程进入）

    fileprivate func openTun(optionsJSON: String) -> Int32 {
        NSLog("[PrismVPN] openTun called with options: \(optionsJSON)")
        writeHeartbeat("openTun")
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

        // 同步等待系统确认网卡挂载（Go 线程阻塞在此，最多 10s）
        let semaphore = DispatchSemaphore(value: 0)
        var settingsError: Error?
        setTunnelNetworkSettings(settings) { error in
            settingsError = error
            semaphore.signal()
        }

        let waitResult = semaphore.wait(timeout: .now() + 10)
        if waitResult == .timedOut {
            NSLog("[PrismVPN] openTun: setTunnelNetworkSettings callback not received in 10s")
            writeHeartbeat("tunTimeout")
            return -1 // 返回 -1 让 Go 内核退出，随后触发启动失败
        }

        if let settingsError = settingsError {
            NSLog("[PrismVPN] openTun: setTunnelNetworkSettings failed: \(settingsError.localizedDescription)")
            return -1
        }
        NSLog("[PrismVPN] openTun: setTunnelNetworkSettings succeeded")
        writeHeartbeat("tun-ok")

        let fd: Int32? = (
            packetFlow.value(forKeyPath: "socket.fileDescriptor") as? Int32
        ) ?? (
            packetFlow.value(forKeyPath: "socket.socketDescriptor") as? Int32
        )
        guard let fd = fd, fd >= 0 else {
            NSLog("[PrismVPN] openTun: failed to get packetFlow fileDescriptor")
            writeHeartbeat("tunFdFail")
            return -1
        }
        NSLog("[PrismVPN] openTun: got packetFlow fd=\(fd)")
        writeHeartbeat("tun-fd")

        // 【最关键的一步】网卡挂载成功且拿到 fd，此时才向 iOS 报告
        // 隧道建立完成——之后才可能有"connected"状态
        self.notifyStartSuccess()
        return fd
    }

    // MARK: - 心跳

    /// 每 3s 刷新心跳：只要扩展进程活着，prism_status.txt 的时间戳就
    /// 持续更新；进程死亡后心跳冻结，主 App 可据此判死
    private func startHeartbeatTimer() {
        heartbeatTimer?.invalidate()
        let t = Timer.scheduledTimer(withTimeInterval: 3.0, repeats: true) { _ in
            writeHeartbeat("alive")
        }
        RunLoop.main.add(t, forMode: .common)
        heartbeatTimer = t
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
    // iOS 网络扩展进程自身的流量默认不进入本进程的 TUN，无需保护
    return 0
}

@_cdecl("prism_under_extension")
private func prismUnderExtension() -> Int32 {
    return 1
}
