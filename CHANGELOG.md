# 更新日志

本项目版本遵循 [语义化版本](https://semver.org/lang/zh-CN/)。
每次发布前更新本文件，并在 Git 打对应 tag。
移动端与桌面端共用同一版本 tag；更新内容按平台分节说明。

## [0.3.7] - 2026-10-07

> 修复 v0.3.4-0.3.6 均未根治的两个问题：订阅节点大量「消失」、测速全部超时。
> 本次以用户真实订阅做了端到端实证（51 节点全部可见、实测延迟正常）。
> 同时优化订阅导入速度、补全主流节点协议，并按「专业区域放专业模块」
> 全面梳理各页面功能归属。

### 共用修复（移动端 + 桌面端）
- **修复订阅节点大量「消失」**：部分订阅的 `proxies` 含全部节点，但
  `proxy-groups` 只引用其中一小部分（用户订阅 51 个节点，组仅引用 10 个
  CB节点），按组展示的节点页导致其余 41 个节点不可见。配置构建器现在将
  未被任何策略组引用的「游离节点」自动补入主选择组（route.final 链末端的
  selector），全部节点可见、可选、可测速；不改变订阅原有的默认选择。
- **修复测速全部超时**：
  - DNS 远端解析器从国内直连被墙的 `https://1.1.1.1/dns-query`
    （kernel.log 实证 `dial tcp 1.1.1.1:443: i/o timeout`）改为国内可达的
    `https://dns.alidns.com/dns-query`，并配置 `address_resolver` 自举解析。
  - 节点服务器域名（如 `node-1.sub.ibuy.eu.cc`）新增 DNS `outbound` 规则，
    强制经国内解析器（223.5.5.5）解析，节点拨号不再依赖任何境外可达性。
- **订阅导入速度优化**：
  - 订阅地址的「自动双栈」与「强制 IPv4」两次尝试由**串行**改为**竞速**：
    IPv6 出口黑洞时原实现要白等 10s 连接超时才发起第二次，现在任一栈先
    成功立即返回，连接超时同时收紧到 8s。
  - 订阅内 `proxy-providers` 由**逐个串行下载**改为**全部并发下载**，
    N 个 provider 的总耗时从 N 倍 RTT 降为约等于最慢的一个；
    全链路增加毫秒级耗时日志。
- **补全主流节点协议**：
  - Clash 订阅新增 `wireguard`、`socks5`、`http/https` 节点转换（此前这些
    节点会被静默跳过）；WireGuard 本端裸 IP 自动补全 /32、/128 前缀，
    支持 MTU、reserved、预共享密钥。
  - 分享链接新增 `socks5://`（兼容 userinfo 与 query 两种鉴权写法）。
  - 注：SSR 已被 sing-box 在 1.6.0 移除（内核注册直接返回错误），
    无法支持；当前流行协议 VLESS/Reality、Hysteria2、TUIC、Trojan 等均已支持。
- 新增 5 个回归测试（累计 Rust 20 个 / 前端 35 个）：协议转换、
  socks5 链接解析、游离节点补挂、域名解析规则、真实订阅全节点覆盖校验。

### 移动端（iOS / Android）
- 设置页隐藏移动端不成立的选项：「系统代理」接入模式、「设置系统代理」
  开关、「开机自启动」及「以管理员身份运行」提示；接入模式仅保留 TUN。
- **已知架构限制（将在后续版本解决）**：移动端系统不存在 HTTP 代理通道，
  普通 App 无法把自身设为系统代理，只有创建系统 VPN（Android VpnService /
  iOS NEPacketTunnelProvider）才能真正接管流量。当前移动端启动后仅在本地
  监听代理端口，系统流量不会经过它；需增加 VPN 扩展（基于 sing-box libbox）。

### 桌面端（Windows / macOS / Linux）
- **移除节点页重复的「启动内核」按钮**：启停入口统一收敛到仪表盘，
  节点页空状态改为引导「前往仪表盘启动」；仪表盘按钮文案简化为「启动 / 停止」。

### 验证记录
- 真实订阅：clash API 返回 51 个 VLESS 节点；主选择组 55 个成员
  （含全部节点）；实测 `CB节点 10` 437ms、`UCN优选12` 1093ms。
- 新协议配置通过 `sing-box check` 运行期校验（wireguard/socks/http 出站）。
- 五目标交叉编译：Windows x64、macOS x64/ARM、iOS、Android 全部通过。
- 个别节点返回 503 为节点服务端当下不可用（机场节点质量问题），非客户端问题。

## [0.3.6] - 2026-10-07

> 0.3.5 桌面端正常出包，但移动端 CI 编译失败，移动端产物缺失；
> 本版本修复该编译错误，内容与 0.3.5 完全一致，五端重新出包。

### 移动端（iOS / Android）
- **修复 v0.3.5 移动端编译失败**：`platform/mod.rs` 顶部对
  `windows/macos/linux::ProxyBackup` 的 re-export 未加平台门控，
  移动端目标无这些模块，报 E0432 无法解析导入。现 `ProxyBackup`
  类型统一定义在 `mod.rs`，各平台模块通过 `super::ProxyBackup` 引用，
  移动端 `get_system_proxy` 返回空备份（无系统代理概念，运行时为 no-op）。
- 功能与 0.3.5 一致：仪表盘「今日流量」会话内实时统计、关闭清零。

### 桌面端（Windows / macOS / Linux）
- 无功能变更；上述重构对桌面端行为无影响，随同一 tag 重新出包。
- 功能与 0.3.5 一致：系统代理独占（启动备份/停止恢复）、关闭最小化到托盘。

## [0.3.5] - 2026-10-07

### 移动端（iOS / Android）
- **仪表盘简化**：隐藏「内存占用」卡片，替换为「今日流量」——实时显示当前会话
  累计流量（关闭 APP 后自动清零），无持久化存储负担。

### 桌面端（Windows / macOS / Linux）
- **系统代理独占性**：启动时备份当前系统代理设置，停止/退出时恢复原设置，
  确保只有当前运行的 Prism 的代理配置生效，不干扰其他代理软件。
- 点关闭按钮自动最小化到系统托盘；托盘菜单提供「显示主窗口」「退出」。

### 共用修复
- 修复节点测速全部失败：`group_url_test` 改为逐节点调用 `/proxies/{name}/delay`，
  解决 sing-box 1.11.3 `/group` 端点不返回成员延迟的问题。
- 修复订阅节点丢失：新增 hysteria（hy）协议支持；base64 解码兼容 URL-safe
  字符集；前端过滤嵌套策略组，避免把组当成节点展示。
- 移除「手动代理」接入模式：旧配置自动映射为系统代理，避免用户误选后无法上网。
- Pro 功能调整：连接管理改为免费；Pro 保留 TUN 模式、多订阅、规则自定义。

## [0.3.3] - 2026-10-07

### 移动端（iOS / Android）
- **修复节点测速全部失败**：`group_url_test` 原先调用 `/group/{name}/delay`，
  但 sing-box 1.11.3 对该端点只返回组本身延迟而非成员 map，导致前端把全部
  节点标记为超时。改为逐个节点调用 `/proxies/{name}/delay` 汇总，结果可靠。
- **修复订阅节点丢失**：新增 hysteria（hy）协议解析支持；base64 解码改为
  尝试标准/URL-safe 两种字符集；URI 解析增加节点计数日志便于排查。
- **移除「手动代理」接入模式**：旧配置中的 `ruleOnly` 自动映射为系统代理，
  避免用户误选后无法上网。Pro 功能保留 TUN/多订阅/规则自定义，
  连接管理改为免费功能。

### 桌面端（Windows / macOS / Linux）
- 同上三项修复。
- 接入模式选项从三个减为两个（系统代理 / TUN），UI 更简洁。

## [0.3.3] - 2026-10-07

### 移动端（iOS / Android）
- **修复 v0.3.2 Release 缺失移动端产物**：`mobile.yml` 此前仅在 push main 时
  构建为 workflow artifact（不进 Release），导致 tag Release 只有桌面端。
  现改为 tag 触发，并将已签名 Android APK 与 TrollStore IPA 通过
  `softprops/action-gh-release` 上传到同一 Release。

### 桌面端（Windows / macOS / Linux）
- 无功能变更，随同一 tag `v0.3.3` 出包。

## [0.3.2] - 2026-10-07

> 0.3.1 因移动端构建失败未发布，其 tag 作废，全部内容并入本版本。

### 移动端（iOS / Android）
- **修复 0.3.1 构建失败**（此前 0.3.0 起移动端无法出包）：
  - iOS：`kernel.rs` 中 `clear_kernel_pid(self.pid)` 缺少 `not(ios, android)`
    门控，导致 iOS 编译报 `cannot find function clear_kernel_pid`
  - Android：Go 在 android 目标不支持 `-buildmode=c-archive`，改用
    `-buildmode=c-shared` 产出四架构 `.so`（arm64-v8a / armeabi-v7a /
    x86 / x86_64），`build.rs` 以 dylib 链接，CI 拷入 `jniLibs/<abi>/` 随 APK 打包
- **geo 数据库改为编译期内嵌**（iOS + Android 统一方案）：实测 Tauri iOS
  不会把 bundle resources 打进 .app（数组/映射写法均无效），APK assets
  亦非真实文件系统；改用 `include_bytes!` 内嵌进主二进制，启动时写入
  工作目录。数据库文件缺失即编译失败，天然成为构建期硬校验
- Android 内嵌内核完整落地：sing-box 1.11.3 进程内运行，四架构全覆盖

### 桌面端（Windows / macOS / Linux）
- 本版本桌面端无功能变更，随同一 tag 正常出包
- 沿用 0.3.0 的零运行时下载方案：内核 sidecar + geo 数据库随包分发，
  缺失直接报错（应用不联网下载任何内核文件），CI 打包前硬校验

## [0.3.1] - 2026-10-07（构建失败，未发布）

- 尝试修复 0.3.0 移动端构建失败（iOS cfg 门控 + Android c-shared），
  但 iOS geo 数据库 bundle 问题未在此版本内解决，tag 作废。

## [0.3.0] - 2026-10-07

### 核心变更
- **全平台零运行时下载**：内核（sing-box）与 geoip/geosite 数据库必须随安装包分发，
  任何平台缺失时直接报错，绝不依赖用户网络补齐（用户网络可能无法访问 GitHub）
- **Android 内嵌内核**：与 iOS 相同，sing-box 以 c-archive 静态链接进主程序
  （四架构 arm64/arm/386/amd64），不再有 sidecar 与运行时下载
- **Android geo 数据库编译期内嵌**：APK assets 非真实文件系统，
  改用 `include_bytes!` 在编译期打入二进制，启动时写入工作目录
- **桌面端删除 `kernel_download.rs`**：缺失内核直接提示重新安装

### UI 防混淆
- 设置页「运行模式」→「接入模式」，与「出站模式」明确区分两个独立维度
- 消除重名：RunMode 的 `TUN 全局`→`TUN 虚拟网卡`、`仅规则`→`手动代理`；
  OutboundMode 的 `规则`→`规则分流`、`全局`→`全局代理`、`直连`→`全部直连`
- 内核校验按钮图标由 `Download` 改为 `ShieldCheck`（不再下载）
- 连接管理页 ProGate 包裹整页（原工具栏在门外可点但表格被锁）
- 规则页策略输入补充 direct/block/策略组名 提示
- 节点页空状态按钮「更新订阅」→「管理订阅」
- 模式切换增加 toast 反馈（运行中=已重启生效，未运行=下次启动生效）

### 测试
- 引入 Vitest + Testing Library 测试基础设施
- 抽取 modes / latency / ruleType / time 纯逻辑模块
- 35 个单元测试覆盖模式标签唯一性、延迟/规则类型映射、时间与字节格式化

### CI
- 桌面 `build.yml` 打包前硬校验 sidecar + geoip.db + geosite.db 均已就位
- Android `mobile.yml` 新增 Go 环境、geo 数据库抓取、四架构 c-archive 构建

## [0.2.0] - 历史版本
- 初始公开发布版本
