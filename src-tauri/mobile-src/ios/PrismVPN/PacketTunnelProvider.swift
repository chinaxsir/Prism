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
//
// build 150 修复（build=149 实测 Code=12 且 hb=无）：
//   根因 1：TrollStore 覆盖安装只杀主 App 不杀后台驻留的旧扩展进程，
//   旧 PrismVPN 挂在 nesessionmanager 上但磁盘二进制已被新版覆盖，
//   XPC 调用失败 → Code=12 且 startTunnel 根本不被拉起（hb=无）。
//   修复：stopTunnel 完成后 exit(0) 干净退出，杜绝僵尸进程残留。
//   根因 2：build 149 向 dns.servers 注入 detour="direct" 在 sing-box
//   新版直接报错 "detour to an empty direct outbound makes no sense"。
//   修复：回退该注入，保持 build=147/148 已验证连通的最小清洗。

import Foundation
import NetworkExtension
import Darwin

private let prismAppGroupID = "group.com.prism.proxy"
private var prismProviderRef: PacketTunnelProvider?

// MARK: - 物理网卡索引（缓存 + 线程安全刷新）

/// prismProtectSocket 可能被 sing-box 内核在多个 Go 线程高频并发调用，
/// 不能每次回调都跑 getifaddrs（系统调用风暴 + 并发遍历链表有崩溃
/// 风险）。启动时与低频定时刷新缓存，回调里无锁读原子变量
private var cachedPhysicalIfIndex: Int32 = 0 // 0 = 未绑定（直连默认路由）
/// 物理网卡名（en0 / pdp_ip0...），sanitize 用它写 route.default_interface
/// 与出站 bind_interface（build 151：显式绑定，不依赖回调链路）
private var cachedPhysicalIfName: String = ""

private func refreshPhysicalInterfaceIndex() {
    var ifaddrPtr: UnsafeMutablePointer<ifaddrs>?
    guard getifaddrs(&ifaddrPtr) == 0, let firstAddr = ifaddrPtr else { return }
    defer { freeifaddrs(ifaddrPtr) }

    var wifiIdx: Int32 = 0
    var cellIdx: Int32 = 0
    var otherIdx: Int32 = 0
    var wifiName = ""
    var cellName = ""
    var otherName = ""

    var ptr: UnsafeMutablePointer<ifaddrs>? = firstAddr
    while let current = ptr {
        let flags = Int32(current.pointee.ifa_flags)
        let isUp = (flags & IFF_UP) != 0 && (flags & IFF_RUNNING) != 0
        let isLoopback = (flags & IFF_LOOPBACK) != 0

        if isUp && !isLoopback, let addr = current.pointee.ifa_addr {
            let family = Int32(addr.pointee.sa_family)
            if family == AF_INET || family == AF_INET6 {
                let name = String(cString: current.pointee.ifa_name)
                if !name.hasPrefix("utun") && !name.hasPrefix("ipsec") && !name.hasPrefix("ppp")
                    && !name.hasPrefix("llw") && !name.hasPrefix("awdl")
                {
                    let idx = Int32(if_nametoindex(name))
                    if idx > 0 {
                        if name == "en0" {
                            wifiIdx = idx
                            wifiName = name
                        } else if name.hasPrefix("en") {
                            if wifiIdx == 0 { wifiIdx = idx; wifiName = name }
                        } else if name == "pdp_ip0" {
                            cellIdx = idx
                            cellName = name
                        } else if name.hasPrefix("pdp_ip") {
                            if cellIdx == 0 { cellIdx = idx; cellName = name }
                        } else if otherIdx == 0 {
                            otherIdx = idx
                            otherName = name
                        }
                    }
                }
            }
        }
        ptr = current.pointee.ifa_next
    }

    if wifiIdx != 0 {
        cachedPhysicalIfIndex = wifiIdx
        cachedPhysicalIfName = wifiName
    } else if cellIdx != 0 {
        cachedPhysicalIfIndex = cellIdx
        cachedPhysicalIfName = cellName
    } else if otherIdx != 0 {
        cachedPhysicalIfIndex = otherIdx
        cachedPhysicalIfName = otherName
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

    /// 主 App 传入的 config_path 所在目录（build 145 方案）：主 App 把
    /// prism_config.json 写进这个目录，扩展能读到它就证明目录 100% 可读写，
    /// 是比 containerURL 更可靠的共享目录（containermanagerd 对扩展可能
    /// 返回 nil / 沙盒拦截 group 容器）。startTunnel 读到 config_path 时设置
    private var resolvedSharedDirURL: URL?

    /// openTun 内部最近一次失败原因（build 146 方案）：由 failStart 拼接
    /// 输出，防止被 Go 侧笼统的 "platform open_tun failed" 覆盖；
    /// 最近心跳阶段用于 failStart 时保留精确卡点
    private var lastOpenTunError: String?
    private var lastHeartbeatStage = "init"

    // MARK: - 共享目录与诊断写入

    /// 共享目录候选列表（按可靠度排序）：config_path 父目录 → group 容器
    /// → 用户 Preferences（无沙盒权限时的最后兜底，build 150 方案保留）。
    /// 心跳/错误对所有候选逐一写入，哪个成功算哪个，主 App 侧同样多目录读取
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
        NSLog("[PrismVPN] ERROR: \(message)")
        for dir in sharedDirURLs() {
            let url = dir.appendingPathComponent("prism_error.txt")
            try? message.write(to: url, atomically: true, encoding: .utf8)
        }
    }

    private func clearSharedError() {
        lastOpenTunError = nil
        for dir in sharedDirURLs() {
            try? FileManager.default.removeItem(
                at: dir.appendingPathComponent("prism_error.txt")
            )
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

    // MARK: - 启动隧道

    override func startTunnel(
        options: [String: NSObject]?,
        completionHandler: @escaping (Error?) -> Void
    ) {
        prismProviderRef = self
        writeHeartbeat("start")
        clearSharedError()

        // config_path 所在目录 = 主 App 写 prism_config.json 的目录，
        // 能读到本文件就证明该目录可读写 → 作为共享目录首选
        if let configPath = (protocolConfiguration as? NETunnelProviderProtocol)?
            .providerConfiguration?["config_path"] as? String,
           !configPath.isEmpty
        {
            resolvedSharedDirURL = URL(fileURLWithPath: configPath).deletingLastPathComponent()
            writeHeartbeat("start-gotPath")
        }

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
                failStart(code: 3, message: "无法读取共享配置文件: \(error.localizedDescription)", completionHandler: completionHandler)
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
        // kernel.log/cache.db 主 App 侧诊断可见），group 容器兜底
        let baseDir = resolvedSharedDirURL
            ?? FileManager.default.containerURL(
                forSecurityApplicationGroupIdentifier: prismAppGroupID)
        guard let workBase = baseDir else {
            failStart(code: 3, message: "共享目录不可用（config_path 与 App Group 均不可用）", completionHandler: completionHandler)
            return
        }
        let workDir = workBase.appendingPathComponent("prism_work", isDirectory: true)

        // 在后台串行队列启动内核，释放主线程以接收 setTunnelNetworkSettings 的系统回调
        vpnQueue.async { [weak self] in
            guard let self = self else { return }

            do {
                try FileManager.default.createDirectory(at: workDir, withIntermediateDirectories: true)
                try? self.copyGeoDatabases(to: workDir)

                // 物理网卡索引/名字预缓存：先于 sanitize（要写
                // route.default_interface 与出站 bind_interface），也供
                // prismProtectSocket 回调无锁读取
                refreshPhysicalInterfaceIndex()

                // 删除旧 cache.db（build 156 用户方案）：配合 cache_file
                // enabled=false，彻底防止恢复旧的超时节点
                try? FileManager.default.removeItem(
                    at: workDir.appendingPathComponent("cache.db")
                )

                let sanitizedConfig = self.sanitizeConfigForExtension(configText, workDir: workDir)

                let callbacks = prism_vpn_cbs(
                    open_tun: prismOpenTun,
                    protect_socket: prismProtectSocket,
                    under_extension: prismUnderExtension
                )
                PrismVPNSetCallbacks(callbacks)
                self.writeHeartbeat("calling-PrismVPNStart")

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

                NSLog("[PrismVPN] PrismVPNStart succeeded, tunnel ready")
                self.writeHeartbeat("running")
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
        stateLock.lock()
        currentState = .stopped
        stateLock.unlock()
        // build 146 方案：拼接 openTun 内部记录的具体原因（Go 的
        // "platform open_tun failed" 只有笼统一句），并保留最近心跳阶段
        var fullMessage = message
        if let tunErr = lastOpenTunError, !tunErr.isEmpty {
            fullMessage = "\(message) [openTun原因: \(tunErr)]"
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
            self?.writeHeartbeat("alive")
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
            self?.writeHeartbeat("stopped:\(reason.rawValue)")
            if let self = self {
                self.stateLock.lock()
                self.currentState = .stopped
                self.stateLock.unlock()
            }
            DispatchQueue.main.async {
                completionHandler()
            }
            // 【build 150 根因 1 修复】干净退出：iOS 网络扩展进程结束任务后
            // 系统通常保留进程对象一段时间，TrollStore 覆盖安装只杀主 App
            // 不杀扩展——旧版 PrismVPN 进程会继续挂在 nesessionmanager 上，
            // 占住 VPN 会话导致新版 startTunnel 根本不被拉起（Code=12 且
            // hb=无）。completionHandler 之后显式 exit，杜绝僵尸进程
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) {
                NSLog("[PrismVPN] stopTunnel completed, exiting cleanly")
                exit(0)
            }
        }
    }

    // MARK: - OpenTun（Go 回调进入）

    fileprivate func openTun(optionsJSON: String) -> Int32 {
        NSLog("[PrismVPN] openTun called with options: \(optionsJSON)")
        writeHeartbeat("openTun")
        guard let data = optionsJSON.data(using: .utf8),
              let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else {
            lastOpenTunError = "optionsJSON 解析失败: \(optionsJSON.prefix(200))"
            writeHeartbeat("tunJsonErr")
            return -1
        }

        // build 146 方案：MTU 容错——Go 传 0/超大值时系统会直接拒绝，
        // 规范到 [1280, 9000]，异常回退 1400
        let rawMtu = (json["mtu"] as? NSNumber)?.int32Value ?? 1400
        let mtu: Int32 = (rawMtu >= 1280 && rawMtu <= 9000) ? rawMtu : 1400

        let settings = NEPacketTunnelNetworkSettings(tunnelRemoteAddress: "127.0.0.1")
        settings.mtu = NSNumber(value: mtu)

        // DNS：拦截全局 DNS 进入隧道由 sing-box 处理。matchDomains=[""]
        // 才能全局接管系统 DNS；sing-box 传来的 dns_server 为空或 "local"
        // 时兜底公共 DNS，避免虚拟网卡无 DNS 栈
        let dnsServer = (json["dns_server"] as? String)
            ?? ((json["dns_servers"] as? [String])?.first)
            ?? "1.1.1.1"
        let cleanDNS = (dnsServer.isEmpty || dnsServer == "local") ? "1.1.1.1" : dnsServer
        let dnsSettings = NEDNSSettings(servers: [cleanDNS, "8.8.8.8"])
        dnsSettings.matchDomains = [""]
        settings.dnsSettings = dnsSettings

        // IPv4 地址与路由（build 150 回退 build=147/148 已验证逻辑）：
        // includedRoutes 按 Go 传入的 route_address 生成（空则全局默认）；
        // 不再加 excludedRoutes（149 版对回环排除的猜测在实测中无必要，
        // 且多余路由可能引发系统侧行为差异）
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

        // IPv6：可选栈，仅有效前缀参与（sing-box 传 inet6_address 时启用）
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

        // 应用网络配置到系统虚拟网卡：10s 超时；若因 IPv6 栈被系统拒绝，
        // 清掉 IPv6 仅用 IPv4 重试一次（build 146 方案）
        if let settingsError = applyNetworkSettingsSync(settings) {
            settings.ipv6Settings = nil
            if let retryError = applyNetworkSettingsSync(settings) {
                lastOpenTunError = "setTunnelNetworkSettings失败: \(retryError)（原始错误: \(settingsError)）"
                writeHeartbeat("tunSetErr")
                return -1
            }
        }
        writeHeartbeat("tun-ok")

        // 取 utun 文件描述符：iOS 16+ 上 KVC 路径（socket.fileDescriptor）
        // 会返回 nil，需回退到 XNU 内核 utun 接口扫描（findTunnelFileDescriptor）
        guard let fd = findTunnelFileDescriptor(), fd >= 0 else {
            lastOpenTunError = "无法获取 utun 文件描述符（KVC 与 getsockopt 内核扫描均未命中）"
            writeHeartbeat("tunFdFail")
            return -1
        }
        NSLog("[PrismVPN] openTun succeeded, fd=\(fd)")
        writeHeartbeat("tun-fd:\(fd)")
        return fd
    }

    /// 同步应用虚拟网卡设置，10s 超时；返回 nil = 成功，非 nil = 错误描述
    private func applyNetworkSettingsSync(_ settings: NEPacketTunnelNetworkSettings) -> String? {
        let semaphore = DispatchSemaphore(value: 0)
        var settingsError: Error?
        setTunnelNetworkSettings(settings) { error in
            settingsError = error
            semaphore.signal()
        }
        if semaphore.wait(timeout: .now() + 10) == .timedOut {
            return "10s超时"
        }
        return settingsError?.localizedDescription
    }

    /// 获取 TUN 虚拟网卡文件描述符（build 146 方案核心）：
    /// 1) KVC 路径在 iOS 16+ 重构后返回 nil；
    /// 2) 回退扫描当前进程 FD 0...1024，用 getsockopt(fd, SYSPROTO_CONTROL,
    ///    UTUN_OPT_IFNAME) 直接从 XNU 内核匹配 "utun*" 虚拟网卡——
    ///    sing-box 官方 SFI 客户端的 LibboxGetTunnelFileDescriptor 同款兜底
    private func findTunnelFileDescriptor() -> Int32? {
        if let fd = (packetFlow.value(forKeyPath: "socket.fileDescriptor") as? Int32)
            ?? (packetFlow.value(forKeyPath: "socket.socketDescriptor") as? Int32),
           fd >= 0
        {
            return fd
        }

        let sysprotoControl: Int32 = 2  // SYSPROTO_CONTROL
        let utunOptIfname: Int32 = 2    // UTUN_OPT_IFNAME
        var matchedFd: Int32?
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

    // MARK: - 配置清洗（保持 build=147/148 已验证连通的最小逻辑）

    /// 配置清洗（build 156 用户模板完整版）：
    /// 1. tun 入站：开 sniff + sniff_override_destination（协议探测支撑
    ///    DNS 劫持与 QUIC 屏蔽规则）
    /// 2. 出站识别（dns/block/主代理组）+ bind_interface 白名单绑定 +
    ///    确保 dns-out/block/direct 出站存在
    /// 3. DNS 整体重写：dns-remote = tcp://1.1.1.1 走主代理组（走 VLESS
    ///    TCP，无 UDP 支持也能解析），dns-direct = 223.5.5.5 走 direct；
    ///    出站自身域名解析（outbound any）固定走 dns-direct，杜绝
    ///    「先连节点才能解析节点域名」死锁
    /// 4. route：DNS 劫持（protocol dns / port 53）、QUIC 屏蔽（protocol
    ///    quic / udp 443 → block）、私网直连规则置顶；auto_detect_interface
    ///    关闭 + default_interface 显式绑定物理网卡
    /// 5. experimental.cache_file：enabled=false（严禁写 store_selected——
    ///    1.11.3 无此字段，严格 JSON 校验直接拒配置）
    /// 6. log 恒开 box.log
    private func sanitizeConfigForExtension(_ rawJSON: String, workDir: URL) -> String {
        guard let data = rawJSON.data(using: .utf8),
              var root = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else {
            return rawJSON
        }

        let phyName = cachedPhysicalIfName

        // 1. tun 入站适配 iOS NetworkExtension + 协议探测
        var inbounds = (root["inbounds"] as? [[String: Any]]) ?? []
        var hasTun = false
        for i in inbounds.indices {
            if (inbounds[i]["type"] as? String) == "tun" {
                hasTun = true
                inbounds[i].removeValue(forKey: "interface_name")
                inbounds[i]["strict_route"] = false
                inbounds[i]["auto_route"] = true
                inbounds[i]["mtu"] = 1400
                inbounds[i]["sniff"] = true
                inbounds[i]["sniff_override_destination"] = true
                if inbounds[i]["inet4_address"] == nil && inbounds[i]["address"] == nil {
                    inbounds[i]["inet4_address"] = "172.19.0.1/30"
                }
            }
        }
        if !hasTun {
            inbounds.insert([
                "type": "tun",
                "tag": "tun-in",
                "inet4_address": "172.19.0.1/30",
                "mtu": 1400,
                "auto_route": true,
                "strict_route": false,
                "sniff": true,
                "sniff_override_destination": true,
            ], at: 0)
        }
        root["inbounds"] = inbounds

        // 2. 出站识别 + 显式绑定物理网卡（白名单——策略组/伪出站不绑定）
        var outbounds = (root["outbounds"] as? [[String: Any]]) ?? []
        var dnsOutboundTag = "dns-out"
        var blockOutboundTag = "block"
        var mainProxyTag = ""
        var foundDns = false
        var foundBlock = false
        var hasDirectOutbound = false
        let realOutboundTypes: Set<String> = [
            "direct", "vless", "vmess", "trojan", "shadowsocks",
            "hysteria", "hysteria2", "tuic", "wireguard",
            "http", "socks", "ssh", "anytls",
        ]
        for i in outbounds.indices {
            let t = (outbounds[i]["type"] as? String) ?? ""
            let tag = (outbounds[i]["tag"] as? String) ?? ""
            if t == "dns" && !foundDns && !tag.isEmpty {
                dnsOutboundTag = tag
                foundDns = true
            } else if t == "block" && !foundBlock && !tag.isEmpty {
                blockOutboundTag = tag
                foundBlock = true
            } else if t == "direct" && !hasDirectOutbound {
                hasDirectOutbound = true
            } else if t == "selector" && mainProxyTag.isEmpty && !tag.isEmpty {
                mainProxyTag = tag
            }
            if realOutboundTypes.contains(t) {
                outbounds[i]["bind_interface"] = phyName
            }
        }
        // 确保路由规则引用的出站真实存在（缺失则启动报错——用户模板防踩坑）
        if !hasDirectOutbound {
            var directObj: [String: Any] = ["type": "direct", "tag": "direct"]
            if !phyName.isEmpty {
                directObj["bind_interface"] = phyName
            }
            outbounds.append(directObj)
        }
        if !foundDns {
            outbounds.append(["type": "dns", "tag": "dns-out"])
            dnsOutboundTag = "dns-out"
        }
        if !foundBlock {
            outbounds.append(["type": "block", "tag": "block"])
            blockOutboundTag = "block"
        }
        root["outbounds"] = outbounds

        // 3. route：显式绑定 + DNS 劫持/QUIC 屏蔽/私网直连规则置顶
        var route = (root["route"] as? [String: Any]) ?? [:]
        // route.final 由 config_builder 指向主选择组，覆盖首 selector 识别
        if let finalTag = route["final"] as? String, !finalTag.isEmpty {
            mainProxyTag = finalTag
        }
        // 显式 bind_interface 走 sing-box 拨号器第一分支（已核实 1.11.3
        // 无与 auto_detect_interface 的冲突校验）；这里按用户模板关闭
        // auto_detect_interface，绑定完全由 bind_interface/default_interface
        // 承担
        route["auto_detect_interface"] = false
        if !phyName.isEmpty {
            route["default_interface"] = phyName
        }
        var rules = (route["rules"] as? [[String: Any]]) ?? []
        // 先移除生成器可能已写入的 DNS 劫持规则，避免重复
        rules.removeAll { rule in
            (rule["protocol"] as? [String])?.contains("dns") == true
        }
        let prependRules: [[String: Any]] = [
            // DNS 劫持：协议探测命中的 DNS、任意端口 53 → dns-out 出站
            ["protocol": ["dns"], "outbound": dnsOutboundTag],
            ["port": 53, "outbound": dnsOutboundTag],
            // QUIC 屏蔽：阻断 UDP 443 强制回落 TCP（VLESS 节点 UDP 支持不稳）
            ["protocol": ["quic"], "outbound": blockOutboundTag],
            ["port": 443, "network": "udp", "outbound": blockOutboundTag],
            // 私网直连
            ["ip_is_private": true, "outbound": "direct"],
        ]
        rules.insert(contentsOf: prependRules, at: 0)
        route["rules"] = rules
        // geoip/geosite/rule_set 本地路径重定向
        if var geoip = route["geoip"] as? [String: Any], geoip["path"] != nil {
            geoip["path"] = workDir.appendingPathComponent("geoip.db").path
            route["geoip"] = geoip
        }
        if var geosite = route["geosite"] as? [String: Any], geosite["path"] != nil {
            geosite["path"] = workDir.appendingPathComponent("geosite.db").path
            route["geosite"] = geosite
        }
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

        // 4. DNS 整体重写（build 155 核心）：远端 DNS 走主代理组 TCP，
        //    出站自身解析走直连 DNS，断网时不阻塞启动
        if !mainProxyTag.isEmpty {
            root["dns"] = [
                "servers": [
                    [
                        "tag": "dns-remote",
                        "address": "tcp://1.1.1.1",
                        "detour": mainProxyTag,
                        "strategy": "ipv4_only",
                    ],
                    [
                        "tag": "dns-direct",
                        "address": "223.5.5.5",
                        "detour": "direct",
                        "strategy": "ipv4_only",
                    ],
                ],
                "rules": [
                    // 出站自身的域名解析（节点服务器域名等）固定走直连 DNS
                    ["outbound": "any", "server": "dns-direct"],
                ],
                "final": "dns-remote",
                "strategy": "ipv4_only",
            ]
        } else {
            // 无主代理组（纯直连场景）：DNS 全走直连
            root["dns"] = [
                "servers": [
                    ["tag": "dns-direct", "address": "223.5.5.5", "detour": "direct"],
                ],
                "final": "dns-direct",
                "strategy": "ipv4_only",
            ]
        }

        // 5. experimental.cache_file：严禁写 store_selected——sing-box
        //    1.11.3 CacheFileOptions 只有 enabled/path/cache_id/
        //    store_fakeip/store_rdrc，未知字段触发 DisallowUnknownFields
        //    拒绝整份配置（build 155 实证 extErr=create vpn service:
        //    decode config）。改为 enabled=false 禁用缓存，配合 startTunnel
        //    已删除旧 cache.db，不会再恢复 stale 节点
        var experimental = (root["experimental"] as? [String: Any]) ?? [:]
        var cacheFile = (experimental["cache_file"] as? [String: Any]) ?? [:]
        cacheFile.removeValue(forKey: "store_selected")
        cacheFile["enabled"] = false
        cacheFile["path"] = workDir.appendingPathComponent("cache.db").path
        experimental["cache_file"] = cacheFile
        if var clashAPI = experimental["clash_api"] as? [String: Any] {
            clashAPI.removeValue(forKey: "external_ui")
            clashAPI.removeValue(forKey: "external_ui_download_url")
            experimental["clash_api"] = clashAPI
        }
        root["experimental"] = experimental

        // 6. 强制开启 box.log 文件输出（消除诊断 box=无）
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
        NSLog("[PrismVPN] 配置清洗完成（tun+sniff/dns重写/route规则/绑定/store_selected=false/log）")
        return newText
    }

    // MARK: - Geo 数据库

    /// geo 数据库拷贝：扩展包 → 主 App 包兜底；可选资源缺失时仅跳过
    /// 不阻断启动（内核真需要时 sing-box 会给出明确错误）
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

/// 解析 "172.19.0.1/30" 形式的 CIDR 字符串
private func parseCIDR(_ cidr: String) -> Prefix? {
    let parts = cidr.split(separator: "/")
    guard parts.count == 2, let prefix = Int(parts[1]) else { return nil }
    return Prefix(address: String(parts[0]), prefix: prefix)
}

/// 地址前缀解析：支持 [{address, prefix}] 对象数组与 ["x.x.x.x/n"]
/// 字符串数组（sing-box 不同版本 openTun options 格式存在差异，做双兼容）
private func prefixes(_ raw: Any?) -> [Prefix] {
    if let array = raw as? [[String: Any]] {
        return array.compactMap { item in
            guard let address = item["address"] as? String else { return nil }
            if let prefix = (item["prefix"] as? NSNumber)?.intValue {
                return Prefix(address: address, prefix: prefix)
            }
            return parseCIDR(address)
        }
    }
    if let array = raw as? [String] {
        return array.compactMap { parseCIDR($0) }
    }
    if let single = raw as? String {
        return parseCIDR(single).map { [$0] } ?? []
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

/// 关键修复（build 149 方案）：将 sing-box 的所有出站 Socket 强制绑定到
/// 物理网卡 (en0/pdp_ip0)，防止 openTun 设置 0.0.0.0/0 默认路由后出站
/// 流量再次掉进 utun 造成死循环与全部节点超时。回调只读预缓存的网卡
/// 索引（build 150：线程安全，无 getifaddrs 并发风暴）
@_cdecl("prism_protect_socket")
private func prismProtectSocket(_ fd: Int32) -> Int32 {
    guard fd >= 0 else { return 0 }
    let idx = cachedPhysicalIfIndex
    guard idx > 0 else { return 0 }

    let ipBoundIf: Int32 = 25     // IP_BOUND_IF
    let ipv6BoundIf: Int32 = 125  // IPV6_BOUND_IF

    // 按 socket 实际地址族选择绑定选项（未决 socket 回退双栈都绑）
    var addr = sockaddr_storage()
    var addrLen = socklen_t(MemoryLayout<sockaddr_storage>.size)
    var family: Int32 = AF_UNSPEC
    withUnsafeMutablePointer(to: &addr) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { ptr in
            if getsockname(fd, ptr, &addrLen) == 0 {
                family = Int32(ptr.pointee.sa_family)
            }
        }
    }

    var index = idx
    if family == AF_INET6 {
        _ = setsockopt(fd, IPPROTO_IPV6, ipv6BoundIf, &index, socklen_t(MemoryLayout<UInt32>.size))
    } else if family == AF_INET {
        _ = setsockopt(fd, IPPROTO_IP, ipBoundIf, &index, socklen_t(MemoryLayout<UInt32>.size))
    } else {
        if setsockopt(fd, IPPROTO_IP, ipBoundIf, &index, socklen_t(MemoryLayout<UInt32>.size)) != 0 {
            _ = setsockopt(fd, IPPROTO_IPV6, ipv6BoundIf, &index, socklen_t(MemoryLayout<UInt32>.size))
        }
    }
    return 0
}

@_cdecl("prism_under_extension")
private func prismUnderExtension() -> Int32 {
    return 1
}
