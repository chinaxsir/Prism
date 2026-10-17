import Foundation
import NetworkExtension
import Darwin

private let prismAppGroupID = "group.com.prism.proxy"
private var prismProviderRef: PacketTunnelProvider?

/// 缓存当前活跃的物理网卡索引与名称（en0 / pdp_ip0）
private var cachedPhysicalIfIndex: UInt32 = 0
private var cachedPhysicalIfName: String = "en0"

private struct PhysicalInterface {
    let name: String
    let index: UInt32
}

/// 严格判断网卡上的地址是否为真正可路由的 IP（排除 127.0.0.1、169.254.x.x 以及 iOS Wi-Fi 断开后残留的 fe80:: 链路本地地址）
private func isRoutableAddress(_ addrPtr: UnsafePointer<sockaddr>) -> Bool {
    let family = Int32(addrPtr.pointee.sa_family)
    if family == AF_INET {
        return addrPtr.withMemoryRebound(to: sockaddr_in.self, capacity: 1) { sinPtr in
            let ip = UInt32(bigEndian: sinPtr.pointee.sin_addr.s_addr)
            let b0 = (ip >> 24) & 0xff
            let b1 = (ip >> 16) & 0xff
            if b0 == 0 || b0 == 127 { return false }
            if b0 == 169 && b1 == 254 { return false } // 排除 169.254.0.0/16 链路本地地址
            return true
        }
    } else if family == AF_INET6 {
        return addrPtr.withMemoryRebound(to: sockaddr_in6.self, capacity: 1) { sin6Ptr in
            var addr = sin6Ptr.pointee.sin6_addr
            return withUnsafeBytes(of: &addr) { rawBuf -> Bool in
                guard rawBuf.count >= 16 else { return false }
                let b0 = rawBuf[0]
                let b1 = rawBuf[1]
                // 排除 fe80::/10 链路本地地址（Wi-Fi 未连接路由器时 en0 依然保有 fe80:: 地址！）
                if b0 == 0xfe && (b1 & 0xc0) == 0x80 { return false }
                // 排除 :: 和 ::1 回环地址
                var allZero = true
                for i in 0..<15 where rawBuf[i] != 0 { allZero = false; break }
                if allZero && (rawBuf[15] == 0 || rawBuf[15] == 1) { return false }
                // 仅认可 2000::/3 全球单播 IPv6 地址（如三大运营商 4G/5G 的 2408/2409/240e 开头地址）
                return (b0 & 0xe0) == 0x20
            }
        }
    }
    return false
}

/// 获取当前真正联网的物理网卡（Wi-Fi 在家选 en0，出门断 Wi-Fi 自动选蜂窝流量 pdp_ip0）
private func findActivePhysicalInterface() -> PhysicalInterface {
    var ifaddrPtr: UnsafeMutablePointer<ifaddrs>?
    guard getifaddrs(&ifaddrPtr) == 0, let firstAddr = ifaddrPtr else {
        let idx = if_nametoindex("en0")
        return PhysicalInterface(name: "en0", index: idx)
    }
    defer { freeifaddrs(ifaddrPtr) }

    var wifiCandidate: PhysicalInterface?
    var cellCandidate: PhysicalInterface?
    var otherCandidate: PhysicalInterface?

    var ptr: UnsafeMutablePointer<ifaddrs>? = firstAddr
    while let current = ptr {
        let flags = Int32(current.pointee.ifa_flags)
        let isUp = (flags & IFF_UP) != 0 && (flags & IFF_RUNNING) != 0
        let isLoopback = (flags & IFF_LOOPBACK) != 0

        if isUp && !isLoopback, let addr = current.pointee.ifa_addr, isRoutableAddress(addr) {
            let name = String(cString: current.pointee.ifa_name)
            if !name.hasPrefix("utun") && !name.hasPrefix("ipsec") && !name.hasPrefix("ppp") && !name.hasPrefix("llw") && !name.hasPrefix("awdl") && !name.hasPrefix("lo") {
                let idx = if_nametoindex(name)
                if idx > 0 {
                    if name == "en0" {
                        wifiCandidate = PhysicalInterface(name: name, index: idx)
                    } else if name.hasPrefix("pdp_ip") {
                        if cellCandidate == nil || name == "pdp_ip0" {
                            cellCandidate = PhysicalInterface(name: name, index: idx)
                        }
                    } else if otherCandidate == nil {
                        otherCandidate = PhysicalInterface(name: name, index: idx)
                    }
                }
            }
        }
        ptr = current.pointee.ifa_next
    }

    if let wifi = wifiCandidate {
        cachedPhysicalIfIndex = wifi.index
        cachedPhysicalIfName = wifi.name
        return wifi
    }
    if let cell = cellCandidate {
        cachedPhysicalIfIndex = cell.index
        cachedPhysicalIfName = cell.name
        return cell
    }
    if let other = otherCandidate {
        cachedPhysicalIfIndex = other.index
        cachedPhysicalIfName = other.name
        return other
    }
    // 兜底：若 getifaddrs 均未匹配，优先尝试 pdp_ip0 再尝试 en0
    let pdpIdx = if_nametoindex("pdp_ip0")
    if pdpIdx > 0 {
        cachedPhysicalIfIndex = pdpIdx
        cachedPhysicalIfName = "pdp_ip0"
        return PhysicalInterface(name: "pdp_ip0", index: pdpIdx)
    }
    let enIdx = if_nametoindex("en0")
    cachedPhysicalIfIndex = enIdx
    cachedPhysicalIfName = "en0"
    return PhysicalInterface(name: "en0", index: enIdx)
}

final class PacketTunnelProvider: NEPacketTunnelProvider {

    private enum TunnelState {
        case stopped
        case starting
        case running
        case stopping
    }

    private let vpnQueue = DispatchQueue(label: "com.prism.proxy.vpnQueue", qos: .userInitiated)
    private var currentState: TunnelState = .stopped
    private let stateLock = NSLock()
    private var heartbeatTimer: Timer?

    private var resolvedSharedDirURL: URL?
    private var lastOpenTunError: String?
    private var lastHeartbeatStage: String = "init"

    /// 保存当前运行时的工作目录与绑定网卡名，用于在 Wi-Fi <-> 蜂窝流量切换时自动热重载内核
    private var activeWorkDir: URL?
    private var activeBoundInterface: String = ""
    private var isReloadingForNetworkChange: Bool = false

    // MARK: - 生命周期

    /// 【build 169 方案】init() 级最早心跳：系统 pkd 实例化扩展进程的
    /// 瞬间（早于 startTunnel）即写入 "init" 心跳，诊断可精准区分
    /// 「进程未被拉起（hb=无）」与「拉起了但未进 startTunnel」
    override init() {
        super.init()
        prismProviderRef = self
        writeHeartbeat("init")
    }

    // MARK: - 共享目录与诊断写入

    private func sharedDirURLs() -> [URL] {
        var urls: [URL] = []
        if let resolved = resolvedSharedDirURL {
            urls.append(resolved)
        }
        if let groupURL = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: prismAppGroupID) {
            if !urls.contains(groupURL) {
                urls.append(groupURL)
            }
        }
        let prefURL = URL(fileURLWithPath: "/private/var/mobile/Library/Preferences")
        if !urls.contains(prefURL) {
            urls.append(prefURL)
        }
        return urls
    }

    private func writeSharedError(_ message: String) {
        NSLog("[PrismVPN] ERROR: \(message)")
        for dir in sharedDirURLs() {
            let url = dir.appendingPathComponent("prism_error.txt")
            try? message.write(to: url, atomically: true, encoding: .utf8)
        }
    }

    private func clearSharedError() {
        lastOpenTunError = nil
        for dir in sharedDirURLs() {
            try? FileManager.default.removeItem(at: dir.appendingPathComponent("prism_error.txt"))
        }
    }

    private func writeHeartbeat(_ stage: String) {
        lastHeartbeatStage = stage
        let ts = Int(Date().timeIntervalSince1970)
        let text = "\(ts) \(stage)"
        for dir in sharedDirURLs() {
            let url = dir.appendingPathComponent("prism_status.txt")
            try? text.write(to: url, atomically: true, encoding: .utf8)
        }
    }

    private func loadRawConfigText() throws -> String {
        if let configPath = (protocolConfiguration as? NETunnelProviderProtocol)?
            .providerConfiguration?["config_path"] as? String,
           !configPath.isEmpty
        {
            resolvedSharedDirURL = URL(fileURLWithPath: configPath).deletingLastPathComponent()
            return try String(contentsOfFile: configPath, encoding: .utf8)
        } else if let inlineConfig = (protocolConfiguration as? NETunnelProviderProtocol)?
            .providerConfiguration?["config"] as? String
        {
            return inlineConfig
        } else {
            throw NSError(domain: "PrismVPN", code: 1, userInfo: [NSLocalizedDescriptionKey: "缺少配置（providerConfiguration.config_path）"])
        }
    }

    // MARK: - 启动隧道

    override func startTunnel(
        options: [String: NSObject]?,
        completionHandler: @escaping (Error?) -> Void
    ) {
        prismProviderRef = self
        let phy = findActivePhysicalInterface()
        activeBoundInterface = phy.name

        if let configPath = (protocolConfiguration as? NETunnelProviderProtocol)?
            .providerConfiguration?["config_path"] as? String,
           !configPath.isEmpty
        {
            resolvedSharedDirURL = URL(fileURLWithPath: configPath).deletingLastPathComponent()
        }

        clearSharedError()
        writeHeartbeat("start(\(phy.name))")

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
        do {
            configText = try loadRawConfigText()
        } catch {
            failStart(code: 3, message: "无法读取共享配置文件: \(error.localizedDescription)", completionHandler: completionHandler)
            return
        }

        let baseDir = resolvedSharedDirURL
            ?? FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: prismAppGroupID)
            ?? FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
        let workDir = baseDir.appendingPathComponent("prism_work", isDirectory: true)
        activeWorkDir = workDir

        vpnQueue.async { [weak self] in
            guard let self = self else { return }

            do {
                try FileManager.default.createDirectory(at: workDir, withIntermediateDirectories: true)
                try? self.copyGeoDatabases(to: workDir)

                let currentPhy = findActivePhysicalInterface()
                self.activeBoundInterface = currentPhy.name
                let sanitizedConfig = self.sanitizeConfigForExtension(configText, workDir: workDir, phyName: currentPhy.name)

                let callbacks = prism_vpn_cbs(
                    open_tun: prismOpenTun,
                    protect_socket: prismProtectSocket,
                    under_extension: prismUnderExtension
                )
                PrismVPNSetCallbacks(callbacks)
                self.writeHeartbeat("calling-PrismVPNStart(\(currentPhy.name))")

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

                self.stateLock.lock()
                self.currentState = .running
                self.stateLock.unlock()

                self.writeHeartbeat("running(\(currentPhy.name))")
                DispatchQueue.main.async {
                    self.startHeartbeatTimer()
                    completionHandler(nil)
                }
            } catch {
                self.failStart(code: 2, message: error.localizedDescription, completionHandler: completionHandler)
            }
        }
    }

    /// 当检测到物理网络切换（如出门从 Wi-Fi en0 切换到蜂窝流量 pdp_ip0，或回家从 pdp_ip0 切回 en0）时，
    /// 自动热重载 sing-box 内核并绑定到新网卡，彻底解决切流量断网问题
    private func checkAndReloadIfNetworkChanged() {
        stateLock.lock()
        let isRunning = (currentState == .running)
        stateLock.unlock()
        guard isRunning, !isReloadingForNetworkChange else { return }

        let latestPhy = findActivePhysicalInterface()
        guard !latestPhy.name.isEmpty, latestPhy.name != activeBoundInterface else {
            return
        }

        let oldIf = activeBoundInterface
        let newIf = latestPhy.name
        NSLog("[PrismVPN] 检测到物理网卡切换: \(oldIf) -> \(newIf)，正在热重载内核...")
        isReloadingForNetworkChange = true
        activeBoundInterface = newIf
        writeHeartbeat("netSwitch:\(oldIf)->\(newIf)")

        vpnQueue.async { [weak self] in
            guard let self = self, let workDir = self.activeWorkDir else {
                self?.isReloadingForNetworkChange = false
                return
            }
            if let rawConfig = try? self.loadRawConfigText() {
                let updatedConfig = self.sanitizeConfigForExtension(rawConfig, workDir: workDir, phyName: newIf)
                PrismVPNStop()
                let errPtr = updatedConfig.withCString { configPtr in
                    workDir.path.withCString { dirPtr in
                        PrismVPNStart(
                            UnsafeMutablePointer(mutating: configPtr),
                            UnsafeMutablePointer(mutating: dirPtr)
                        )
                    }
                }
                if let errPtr = errPtr {
                    let msg = String(cString: errPtr)
                    PrismKernelFree(errPtr)
                    self.writeSharedError("网络切换(\(oldIf)->\(newIf))重载失败: \(msg)")
                } else {
                    self.writeHeartbeat("running(\(newIf))")
                }
            }
            self.isReloadingForNetworkChange = false
        }
    }

    private func failStart(code: Int, message: String, completionHandler: @escaping (Error?) -> Void) {
        stateLock.lock()
        currentState = .stopped
        stateLock.unlock()

        let fullMessage: String
        if let tunErr = lastOpenTunError, !tunErr.isEmpty {
            fullMessage = "\(message) [openTun原因: \(tunErr)]"
        } else {
            fullMessage = message
        }

        writeSharedError(fullMessage)
        writeHeartbeat("startFailed(\(lastHeartbeatStage))")
        let err = NSError(domain: "PrismVPN", code: code, userInfo: [NSLocalizedDescriptionKey: fullMessage])
        DispatchQueue.main.async {
            completionHandler(err)
        }
    }

    private func startHeartbeatTimer() {
        heartbeatTimer?.invalidate()
        let t = Timer.scheduledTimer(withTimeInterval: 3.0, repeats: true) { [weak self] _ in
            guard let self = self else { return }
            self.checkAndReloadIfNetworkChanged()
            self.writeHeartbeat("alive(\(self.activeBoundInterface))")
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

        writeHeartbeat("stopped:\(reason.rawValue)")

        DispatchQueue.global(qos: .userInitiated).async {
            PrismVPNStop()
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
            completionHandler()
            DispatchQueue.global().asyncAfter(deadline: .now() + 0.2) {
                exit(0)
            }
        }
    }

    // MARK: - OpenTun

    fileprivate func openTun(optionsJSON: String) -> Int32 {
        NSLog("[PrismVPN] openTun options: \(optionsJSON)")
        writeHeartbeat("openTun")
        _ = findActivePhysicalInterface()

        guard let data = optionsJSON.data(using: .utf8),
              let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else {
            lastOpenTunError = "optionsJSON 解析失败: \(optionsJSON.prefix(200))"
            writeHeartbeat("tunJsonErr")
            return -1
        }

        let rawMtu = (json["mtu"] as? NSNumber)?.int32Value ?? 1400
        let mtu: Int32 = (rawMtu >= 1280 && rawMtu <= 9000) ? rawMtu : 1400

        let settings = NEPacketTunnelNetworkSettings(tunnelRemoteAddress: "127.0.0.1")
        settings.mtu = NSNumber(value: mtu)

        let dnsServer = (json["dns_server"] as? String)
            ?? ((json["dns_servers"] as? [String])?.first)
            ?? "1.1.1.1"
        let cleanDNS = dnsServer.isEmpty ? "1.1.1.1" : dnsServer
        let dnsSettings = NEDNSSettings(servers: [cleanDNS, "8.8.8.8"])
        dnsSettings.matchDomains = [""]
        settings.dnsSettings = dnsSettings

        var v4Addresses = prefixes(json["inet4_address"])
        if v4Addresses.isEmpty {
            v4Addresses = prefixes(json["address"])
        }
        if v4Addresses.isEmpty {
            v4Addresses = [Prefix(address: "172.19.0.1", prefix: 30)]
        }

        let ipv4 = NEIPv4Settings(
            addresses: v4Addresses.map(\.address),
            subnetMasks: v4Addresses.map { subnetMask(prefix: $0.prefix) }
        )
        let v4Routes = prefixes(json["inet4_route_address"])
        if v4Routes.isEmpty {
            ipv4.includedRoutes = [NEIPv4Route.default()]
        } else {
            ipv4.includedRoutes = v4Routes.map {
                if $0.prefix <= 0 || $0.address == "0.0.0.0" {
                    return NEIPv4Route.default()
                }
                return NEIPv4Route(destinationAddress: $0.address, subnetMask: subnetMask(prefix: $0.prefix))
            }
        }
        settings.ipv4Settings = ipv4

        let v6Addresses = prefixes(json["inet6_address"]).filter { !$0.address.isEmpty && $0.prefix > 0 }
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
                    if $0.prefix <= 0 || $0.address == "::" {
                        return NEIPv6Route.default()
                    }
                    return NEIPv6Route(destinationAddress: $0.address, networkPrefixLength: NSNumber(value: $0.prefix))
                }
            }
            settings.ipv6Settings = ipv6
        }

        if let err = applyNetworkSettingsSync(settings) {
            settings.ipv6Settings = nil
            if let retryErr = applyNetworkSettingsSync(settings) {
                lastOpenTunError = "setTunnelNetworkSettings失败: \(retryErr) (原始错误: \(err))"
                writeHeartbeat("tunSetErr")
                return -1
            }
        }

        writeHeartbeat("tun-ok")

        guard let fd = findTunnelFileDescriptor(), fd >= 0 else {
            lastOpenTunError = "无法获取 utun 文件描述符"
            writeHeartbeat("tunFdFail")
            return -1
        }

        NSLog("[PrismVPN] openTun succeeded, fd=\(fd)")
        writeHeartbeat("tun-fd:\(fd)")
        return fd
    }

    private func applyNetworkSettingsSync(_ settings: NEPacketTunnelNetworkSettings) -> String? {
        let semaphore = DispatchSemaphore(value: 0)
        var settingsError: Error?
        setTunnelNetworkSettings(settings) { error in
            settingsError = error
            semaphore.signal()
        }
        let waitResult = semaphore.wait(timeout: .now() + 10)
        if waitResult == .timedOut {
            return "10s超时"
        }
        if let settingsError = settingsError {
            return settingsError.localizedDescription
        }
        return nil
    }

    private func findTunnelFileDescriptor() -> Int32? {
        if let fd = (packetFlow.value(forKeyPath: "socket.fileDescriptor") as? Int32)
            ?? (packetFlow.value(forKeyPath: "socket.socketDescriptor") as? Int32),
           fd >= 0
        {
            return fd
        }

        let sysprotoControl: Int32 = 2
        let utunOptIfname: Int32 = 2
        var matchedFd: Int32? = nil

        for fd: Int32 in 0...1024 {
            var buf = [CChar](repeating: 0, count: 64)
            var len = socklen_t(buf.count)
            let ret = getsockopt(fd, sysprotoControl, utunOptIfname, &buf, &len)
            if ret == 0 && len > 0 {
                let name = String(cString: buf)
                if name.hasPrefix("utun") {
                    matchedFd = fd
                }
            }
        }
        return matchedFd
    }

    // MARK: - 配置清洗与物理网卡绑定

    private func sanitizeConfigForExtension(_ rawJSON: String, workDir: URL, phyName: String) -> String {
        guard let data = rawJSON.data(using: .utf8),
              var root = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else {
            return rawJSON
        }

        let cacheDBURL = workDir.appendingPathComponent("cache.db")
        try? FileManager.default.removeItem(at: cacheDBURL)

        // 1. 修正 inbounds 中的 tun 配置
        if var inbounds = root["inbounds"] as? [[String: Any]] {
            for i in inbounds.indices {
                if (inbounds[i]["type"] as? String) == "tun" {
                    inbounds[i].removeValue(forKey: "interface_name")
                    inbounds[i]["strict_route"] = false
                    inbounds[i]["auto_route"] = true
                    inbounds[i]["mtu"] = 1400
                    inbounds[i]["sniff"] = true
                    inbounds[i]["sniff_override_destination"] = true
                }
            }
            root["inbounds"] = inbounds
        }

        // 2. 检查 outbounds：绑定当前活跃物理网卡 (en0 或 pdp_ip0)，收集全部真实代理节点 tag
        var dnsOutboundTag: String? = nil
        var blockOutboundTag: String? = nil
        var hasDirectOutbound = false
        var firstSelectorTag: String? = nil
        var allRealProxyTags: [String] = []

        let proxyOutboundTypes: Set<String> = [
            "vless", "vmess", "trojan", "shadowsocks",
            "hysteria", "hysteria2", "tuic", "wireguard", "http", "socks", "ssh", "anytls"
        ]
        var outbounds = (root["outbounds"] as? [[String: Any]]) ?? []
        for i in outbounds.indices {
            let t = (outbounds[i]["type"] as? String) ?? ""
            let tag = (outbounds[i]["tag"] as? String) ?? ""
            if t == "dns" {
                dnsOutboundTag = tag.isEmpty ? "dns-out" : tag
            } else if t == "block" {
                blockOutboundTag = tag.isEmpty ? "block" : tag
            } else if t == "direct" {
                hasDirectOutbound = true
                if !phyName.isEmpty {
                    outbounds[i]["bind_interface"] = phyName
                }
            } else if t == "selector" && !tag.isEmpty {
                if firstSelectorTag == nil {
                    firstSelectorTag = tag
                }
            } else if proxyOutboundTypes.contains(t) {
                if !tag.isEmpty {
                    allRealProxyTags.append(tag)
                }
                if !phyName.isEmpty {
                    outbounds[i]["bind_interface"] = phyName
                }
            }
        }

        let routeFinal = (root["route"] as? [String: Any])?["final"] as? String
        let mainProxyTag: String = {
            if let rf = routeFinal, !rf.isEmpty, rf != "direct", rf != "block" {
                return rf
            }
            return firstSelectorTag ?? "proxy"
        }()

        // 确保主 selector 包含全部真实代理节点（防止主 selector 节点列表只剩 10 个子分组节点）
        if !allRealProxyTags.isEmpty {
            var foundMainSelector = false
            for i in outbounds.indices {
                let t = (outbounds[i]["type"] as? String) ?? ""
                let tag = (outbounds[i]["tag"] as? String) ?? ""
                if t == "selector" && tag == mainProxyTag {
                    foundMainSelector = true
                    var existing = (outbounds[i]["outbounds"] as? [String]) ?? []
                    for pTag in allRealProxyTags where !existing.contains(pTag) {
                        existing.append(pTag)
                    }
                    outbounds[i]["outbounds"] = existing
                    if let def = outbounds[i]["default"] as? String,
                       (def == "direct" || !existing.contains(def)),
                       let firstReal = allRealProxyTags.first
                    {
                        outbounds[i]["default"] = firstReal
                    }
                }
            }
            if !foundMainSelector && !allRealProxyTags.contains(mainProxyTag) {
                var selObj: [String: Any] = [
                    "type": "selector",
                    "tag": mainProxyTag,
                    "outbounds": allRealProxyTags
                ]
                if let firstReal = allRealProxyTags.first {
                    selObj["default"] = firstReal
                }
                outbounds.insert(selObj, at: 0)
            }
        }

        if !hasDirectOutbound {
            var directObj: [String: Any] = ["type": "direct", "tag": "direct"]
            if !phyName.isEmpty { directObj["bind_interface"] = phyName }
            outbounds.append(directObj)
        }
        if dnsOutboundTag == nil {
            outbounds.append(["type": "dns", "tag": "dns-out"])
            dnsOutboundTag = "dns-out"
        }
        if blockOutboundTag == nil {
            outbounds.append(["type": "block", "tag": "block"])
            blockOutboundTag = "block"
        }
        root["outbounds"] = outbounds

        // 3. 重构 DNS：
        //    - dns-remote 使用 ipv4_only（确保返回给 Safari 的全为 IPv4，100% 流量进入 utun）
        //    - dns-direct 使用 prefer_ipv4（确保蜂窝流量下解析 VLESS 节点域名时同时支持 IPv4 与 IPv6）
        root["dns"] = [
            "servers": [
                [
                    "tag": "dns-remote",
                    "address": "tcp://1.1.1.1",
                    "detour": mainProxyTag,
                    "strategy": "ipv4_only"
                ],
                [
                    "tag": "dns-direct",
                    "address": "223.5.5.5",
                    "detour": "direct",
                    "strategy": "prefer_ipv4"
                ]
            ],
            "rules": [
                [
                    "outbound": "any",
                    "server": "dns-direct"
                ]
            ],
            "final": "dns-remote",
            "strategy": "ipv4_only"
        ]

        // 4. 修正 route
        var route = (root["route"] as? [String: Any]) ?? [:]
        route["auto_detect_interface"] = false
        if !phyName.isEmpty {
            route["default_interface"] = phyName
        }
        route["final"] = mainProxyTag

        var rules = (route["rules"] as? [[String: Any]]) ?? []
        rules.removeAll { rule in
            let p = rule["protocol"]
            return (p as? String) == "dns" || ((p as? [String])?.contains("dns") == true) || (rule["action"] as? String) == "hijack-dns"
        }

        let finalDnsTag = dnsOutboundTag ?? "dns-out"
        let finalBlockTag = blockOutboundTag ?? "block"

        let headerRules: [[String: Any]] = [
            ["protocol": ["dns"], "outbound": finalDnsTag],
            ["port": [53], "outbound": finalDnsTag],
            ["protocol": ["quic"], "outbound": finalBlockTag],
            ["network": ["udp"], "port": [443], "outbound": finalBlockTag],
            ["ip_is_private": true, "outbound": "direct"]
        ]
        rules.insert(contentsOf: headerRules, at: 0)
        route["rules"] = rules

        if var geoip = route["geoip"] as? [String: Any], geoip["path"] != nil {
            geoip["path"] = workDir.appendingPathComponent("geoip.db").path
            route["geoip"] = geoip
        }
        if var geosite = route["geosite"] as? [String: Any], geosite["path"] != nil {
            geosite["path"] = workDir.appendingPathComponent("geosite.db").path
            route["geosite"] = geosite
        }
        if var ruleSets = route["rule_set"] as? [[String: Any]] {
            for i in ruleSets.indices {
                if (ruleSets[i]["type"] as? String) == "local",
                   let oldPath = ruleSets[i]["path"] as? String,
                   oldPath.contains("/Containers/Data/Application/")
                {
                    let fileName = (oldPath as NSString).lastPathComponent
                    let newPath = workDir.appendingPathComponent("rulesets").appendingPathComponent(fileName).path
                    ruleSets[i]["path"] = newPath
                }
            }
            route["rule_set"] = ruleSets
        }
        root["route"] = route

        // 5. 修正 experimental.cache_file
        if var experimental = root["experimental"] as? [String: Any] {
            if var cacheFile = experimental["cache_file"] as? [String: Any] {
                cacheFile.removeValue(forKey: "store_selected")
                cacheFile["enabled"] = false
                cacheFile["path"] = cacheDBURL.path
                experimental["cache_file"] = cacheFile
            }
            if var clashAPI = experimental["clash_api"] as? [String: Any] {
                clashAPI.removeValue(forKey: "external_ui")
                clashAPI.removeValue(forKey: "external_ui_download_url")
                experimental["clash_api"] = clashAPI
            }
            root["experimental"] = experimental
        }

        // 6. 开启 box.log
        var logObj = (root["log"] as? [String: Any]) ?? [:]
        logObj["disabled"] = false
        logObj["level"] = "info"
        logObj["output"] = workDir.appendingPathComponent("box.log").path
        root["log"] = logObj

        guard let newData = try? JSONSerialization.data(withJSONObject: root),
              let newText = String(data: newData, encoding: .utf8)
        else {
            return rawJSON
        }
        return newText
    }

    private func copyGeoDatabases(to workDir: URL) throws {
        let mainAppBundleURL = Bundle.main.bundleURL
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        for name in ["geoip.db", "geosite.db"] {
            let dst = workDir.appendingPathComponent(name)
            if FileManager.default.fileExists(atPath: dst.path) {
                continue
            }
            if let src = Bundle.main.url(forResource: name, withExtension: nil) {
                try? FileManager.default.copyItem(at: src, to: dst)
            } else {
                let fallbackSrc = mainAppBundleURL.appendingPathComponent(name)
                if FileManager.default.fileExists(atPath: fallbackSrc.path) {
                    try? FileManager.default.copyItem(at: fallbackSrc, to: dst)
                }
            }
        }
    }
}

// MARK: - 地址前缀解析

private struct Prefix {
    let address: String
    let prefix: Int
}

private func parseCIDR(_ cidr: String) -> Prefix? {
    let parts = cidr.split(separator: "/")
    guard parts.count == 2,
          let prefix = Int(parts[1])
    else { return nil }
    return Prefix(address: String(parts[0]), prefix: prefix)
}

private func prefixes(_ raw: Any?) -> [Prefix] {
    if let dictArray = raw as? [[String: Any]] {
        return dictArray.compactMap { item in
            if let address = item["address"] as? String {
                if let prefix = (item["prefix"] as? NSNumber)?.intValue ?? (item["prefix"] as? Int) {
                    return Prefix(address: address, prefix: prefix)
                }
                if let parsed = parseCIDR(address) {
                    return parsed
                }
            }
            return nil
        }
    }
    if let strArray = raw as? [String] {
        return strArray.compactMap { parseCIDR($0) }
    }
    if let singleStr = raw as? String {
        if let parsed = parseCIDR(singleStr) {
            return [parsed]
        }
    }
    return []
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
    guard fd >= 0 else { return 0 }
    var idx = cachedPhysicalIfIndex
    if idx == 0 {
        idx = findActivePhysicalInterface().index
    }
    guard idx > 0 else { return 0 }

    let ipBoundIf: Int32 = 25    // IP_BOUND_IF
    let ipv6BoundIf: Int32 = 125 // IPV6_BOUND_IF

    if setsockopt(fd, IPPROTO_IP, ipBoundIf, &idx, socklen_t(MemoryLayout<UInt32>.size)) != 0 {
        _ = setsockopt(fd, IPPROTO_IPV6, ipv6BoundIf, &idx, socklen_t(MemoryLayout<UInt32>.size))
    }
    return 0
}

@_cdecl("prism_under_extension")
private func prismUnderExtension() -> Int32 {
    return 1
}
