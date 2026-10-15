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

/// 主 App 传入的 config_path 所在目录（build 145 方案）：主 App 把
/// prism_config.json 写进这个目录，扩展能读到它就证明目录 100% 可读写，
/// 是比 containerURL 更可靠的共享目录（containermanagerd 对扩展可能
/// 返回 nil / 沙盒拦截 group 容器）。startTunnel 读到 config_path 时设置
private var resolvedSharedDirURL: URL?

/// 共享目录候选列表（按可靠度排序）：config_path 父目录 → group 容器
/// → 用户 Preferences（无沙盒权限时的最后兜底）。心跳/错误对所有候选
/// 逐一写入，哪个成功算哪个，主 App 侧同样多目录读取
private func sharedDirURLs() -> [URL] {
    var urls: [URL] = []
    if let resolved = resolvedSharedDirURL {
        urls.append(resolved)
    }
    if let groupURL = FileManager.default.containerURL(
        forSecurityApplicationGroupIdentifier: prismAppGroupID
    ), !urls.contains(groupURL) {
        urls.append(groupURL)
    }
    urls.append(URL(fileURLWithPath: "/private/var/mobile/Library/Preferences"))
    return urls
}

private func writeSharedError(_ message: String) {
    for dir in sharedDirURLs() {
        let url = dir.appendingPathComponent("prism_error.txt")
        try? message.write(to: url, atomically: true, encoding: .utf8)
    }
}

private func clearSharedError() {
    for dir in sharedDirURLs() {
        try? FileManager.default.removeItem(
            at: dir.appendingPathComponent("prism_error.txt")
        )
    }
}

private func writeHeartbeat(_ stage: String) {
    let ts = Int(Date().timeIntervalSince1970)
    let text = "\(ts) \(stage)"
    for dir in sharedDirURLs() {
        let url = dir.appendingPathComponent("prism_status.txt")
        try? text.write(to: url, atomically: true, encoding: .utf8)
    }
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
            // config_path 所在目录 = 主 App 写 prism_config.json 的目录，
            // 能读到本文件就证明该目录可读写 → 作为共享目录首选
            resolvedSharedDirURL = URL(fileURLWithPath: configPath).deletingLastPathComponent()
            writeHeartbeat("start-gotPath")
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

        // 工作目录优先放 config_path 父目录（与主 App 读写路径 100% 一致，
        // kernel.log/cache.db 主 App 侧诊断可见），group 容器兜底（build 138
        // 实证扩展无沙盒 Documents，.documentDirectory 指向无权限目录）
        let sharedDir = resolvedSharedDirURL
            ?? FileManager.default.containerURL(
                forSecurityApplicationGroupIdentifier: prismAppGroupID
            )
        guard let workBase = sharedDir else {
            failStart(code: 3, message: "共享目录不可用（config_path 与 App Group 均不可用）", completionHandler: completionHandler)
            return
        }
        let workDir = workBase.appendingPathComponent("prism_work", isDirectory: true)

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

                // build 146 方案第 1 步：清洗配置中指向主 App 私有容器的
                // 路径（Rust 生成配置时用主 App app_data_dir 作 cache_file
                // 等路径，build 145 实证扩展进程读它被 Seatbelt 拦截：
                // initialize cache-file ... operation not permitted），
                // 全部重定向到扩展可写的 workDir
                let sanitizedConfig = self.sanitizeConfigForExtension(configText, workDir: workDir)

                // PrismVPNStart 内部会同步回调 openTun，启动后台协程后立即返回
                //（nil = 成功，非 nil = 错误字符串）
                let errorPtr = sanitizedConfig.withCString { configPtr in
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

    /// 将配置中指向主 App 私有容器（/Containers/Data/Application/...）的
    /// 路径统一重定向到扩展可写的 workDir，防止 sing-box 初始化
    /// cache-file/log/external_ui/rule_set 时触发 operation not permitted
    ///（build 145 实证 cache.db 是第一块沙盒铁板，此处一次性清完同类路径）。
    /// JSON 解析失败时原样返回（内核会给出更明确的错误）
    private func sanitizeConfigForExtension(_ rawJSON: String, workDir: URL) -> String {
        guard let data = rawJSON.data(using: .utf8),
              var root = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else {
            return rawJSON
        }

        // 1. experimental.cache_file.path → workDir/cache.db；
        //    clash_api 移除 external_ui（扩展进程内无需下载/挂载 Web UI
        //    静态目录，避免 mkdir 权限报错）
        if var experimental = root["experimental"] as? [String: Any] {
            if var cacheFile = experimental["cache_file"] as? [String: Any] {
                cacheFile["path"] = workDir.appendingPathComponent("cache.db").path
                experimental["cache_file"] = cacheFile
            }
            if var clashAPI = experimental["clash_api"] as? [String: Any] {
                clashAPI.removeValue(forKey: "external_ui")
                clashAPI.removeValue(forKey: "external_ui_download_url")
                experimental["clash_api"] = clashAPI
            }
            root["experimental"] = experimental
        }

        // 2. log.output → workDir/box.log（仅文件输出需要重定向）
        if var logObj = root["log"] as? [String: Any],
           let output = logObj["output"] as? String,
           !output.isEmpty,
           output != "stdout",
           output != "stderr"
        {
            logObj["output"] = workDir.appendingPathComponent("box.log").path
            root["log"] = logObj
        }

        // 3. route.geoip / route.geosite 本地路径 → workDir（geo 数据库
        //    已由 copyGeoDatabases 拷入 workDir）
        if var route = root["route"] as? [String: Any] {
            if var geoip = route["geoip"] as? [String: Any], geoip["path"] != nil {
                geoip["path"] = workDir.appendingPathComponent("geoip.db").path
                route["geoip"] = geoip
            }
            if var geosite = route["geosite"] as? [String: Any], geosite["path"] != nil {
                geosite["path"] = workDir.appendingPathComponent("geosite.db").path
                route["geosite"] = geosite
            }
            // 4. local rule_set：主 App 私有容器内的 .srs 重定向到
            //    workDir/rulesets/<文件名>（主 App 启动前已拷入共享目录）
            if var ruleSets = route["rule_set"] as? [[String: Any]] {
                var changed = false
                for i in ruleSets.indices {
                    if (ruleSets[i]["type"] as? String) == "local",
                       let oldPath = ruleSets[i]["path"] as? String,
                       oldPath.contains("/Containers/Data/Application/")
                    {
                        let fileName = (oldPath as NSString).lastPathComponent
                        ruleSets[i]["path"] = workDir
                            .appendingPathComponent("rulesets", isDirectory: true)
                            .appendingPathComponent(fileName).path
                        changed = true
                    }
                }
                if changed {
                    route["rule_set"] = ruleSets
                }
            }
            root["route"] = route
        }

        guard let newData = try? JSONSerialization.data(withJSONObject: root),
              let newText = String(data: newData, encoding: .utf8)
        else {
            return rawJSON
        }
        NSLog("[PrismVPN] 配置路径清洗完成（cache_file/log/geo/rule_set → workDir）")
        return newText
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
        // 主 App 包根目录兜底：appex 位于 <主App>/PlugIns/PrismVPN.appex，
        // 上两级即主 App bundle（geo 数据库可能随主 App resources 分发）
        let hostAppBundle = Bundle.main.bundleURL
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        for name in ["geoip.db", "geosite.db"] {
            var src = Bundle.main.url(forResource: name, withExtension: nil)
            if src == nil {
                let hostPath = hostAppBundle.appendingPathComponent(name).path
                if FileManager.default.fileExists(atPath: hostPath) {
                    src = URL(fileURLWithPath: hostPath)
                }
            }
            guard let src = src else {
                // 【build 144 方案】缺 geo 数据库时跳过而非抛异常：
                // 只有用户规则用到 geoip/geosite 时内核才需要它们；
                // 因可选资源缺失直接 failStart 会把「配置根本不用 geo
                // 规则」的正常场景也打死（启动成功率优先）。
                // 若内核真需要而缺失，sing-box 会给出更明确的错误并
                // 写入 prism_error.txt
                NSLog("[PrismVPN] 扩展包与主 App 包均缺少 \(name)，跳过拷贝（未使用 geo 规则时无影响）")
                continue
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
