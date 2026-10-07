# 更新日志

本项目版本遵循 [语义化版本](https://semver.org/lang/zh-CN/)。
每次发布前更新本文件，并在 Git 打对应 tag。
移动端与桌面端共用同一版本 tag；更新内容按平台分节说明。

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
