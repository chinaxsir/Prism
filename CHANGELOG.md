# 更新日志

本项目版本遵循 [语义化版本](https://semver.org/lang/zh-CN/)。
每次发布前更新本文件，并在 Git 打对应 tag。
移动端与桌面端共用同一版本 tag；更新内容按平台分节说明。

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
