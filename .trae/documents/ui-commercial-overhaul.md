# Prism 商用化 UI 重构 + 移动端 Pro 解锁框架

## Context

当前 UI 问题：订阅链接管理混在「规则」页（不合理）；布局只有桌面侧边栏，无移动端适配，不符合商用 APP 标准；IPA（巨魔版）/APK 作为商用分发需要「订阅解锁高级功能」能力。

用户决策（已确认）：
- 订阅管理 → 独立一级「订阅」页；规则页只展示规则
- 解锁方案 → 仅搭框架（Pro 状态、激活 UI、门控逻辑就位；真实校验后续接）
- 付费范围 → 仅移动端（iOS/Android）启用门控；桌面版全功能免费、无激活入口
- 移动端功能划分 → 免费：系统代理 + 单订阅 + 节点选择；Pro：TUN 模式、多订阅、连接管理、规则自定义

## 一、订阅管理迁移（桌面+移动通用）

- 新增 `src/pages/Subscription.tsx`：订阅卡片列表（URL、更新时间、节点数）、添加订阅（URL 输入 + 更新进度事件 `kernel-download://progress` 同款模式已有）、刷新、删除、「当前使用」标记
- `src/pages/Rules.tsx`：删除 L76-158 的订阅管理卡片，保留纯规则列表
- 后端补齐命令（[commands.rs](file:///d:/Prism/src-tauri/src/ipc/commands.rs)）：
  - `delete_subscription(url)` — store.rs 增删记录函数（现有 `upsert_subscription`/`load_subscriptions` 同款模式）
  - `list_subscriptions` 返回增加 `active: bool`（与 profile.yaml 当前来源比对）与节点数（读 profile.yaml 节点计数，已有 `config_builder` 解析能力可复用，简单按 `proxies:` 条目数统计即可）
- 导航：`Sidebar.tsx` 在「节点」后插入「订阅」（图标 `Rss`），`App.tsx` 注册 `/subscription` 路由

## 二、全平台商用 UI（响应式）

- 桌面（`md:` 及以上）：保持 TitleBar + 左侧 Sidebar 布局
- 移动（`<md`）：
  - 新增 `src/components/BottomNav.tsx`：底部 Tab 栏（5+1 项，图标+文字，安全区 padding）
  - `App.tsx`：Sidebar 加 `hidden md:flex`，BottomNav 加 `md:hidden`；TitleBar 移动端隐藏（`hidden md:flex`）
  - 主内容区 `pb-16 md:pb-0` 给底栏留位
- 页面级响应式：Dashboard/Proxies 网格 `grid-cols-1 md:grid-cols-2 xl:grid-cols-3`；Proxies 节点列表移动端单列；各页 padding `p-4 md:p-6`
- 新增轻量组件：`src/components/ui/Toast.tsx`（zustand 单例，成功/错误提示，替代现有的裸 error 文本块）、`EmptyState.tsx`（空列表引导）、`Spinner` 统一加载态
- 各页面接入：操作失败统一 `toast.error(e)`；空订阅/空节点显示 EmptyState 引导跳转

## 三、Pro 解锁框架（仅移动端激活）

- `src/pro/gating.ts`（新增）：
  - `isMobile()`：用已装的 `tauri-plugin-os`（`os.platform()`）判断 ios/android
  - `Feature` 枚举：`Tun`、`MultiSubscription`、`Connections`、`Rules`
  - `isLocked(feature)`：桌面恒 false；移动端读 pro store
- `src/stores/pro.ts`（新增 zustand）：`unlocked: bool`、`activate(code)`、`load()`（启动时从后端读取）
- 后端（框架占位，接口定型后续可换真实校验）：
  - `UserSettings` 增 `pro_unlocked: bool`（serde default，settings.json 已持久化，[state.rs](file:///d:/Prism/src-tauri/src/core/state.rs)）
  - 命令 `activate_pro(code: String) -> Result<bool>`：占位校验（格式 `PRISM-XXXX-XXXX-XXXX` 即通过并持久化），注释标明替换点
  - 命令 `get_pro_status` 或直接复用 `get_settings`
- 门控 UI（仅移动端渲染）：
  - `src/components/ProGate.tsx`：包裹受控内容，锁定时显示毛玻璃遮罩 +「升级 Pro」按钮 → 激活弹窗（输入激活码、调用 `activate_pro`）
  - Sidebar/BottomNav 受控项加 Pro 角标（`Crown` 图标）
  - 受控点：Settings 的 TUN 模式选项、Subscription 页第 2 个订阅添加、Connections 页、Rules 页
  - 桌面版 `isMobile()=false` 时所有门控透明穿透，无任何 UI 痕迹

## 四、顺手收尾

- git push 上次 128 失败（认证）：本次改动完成后重试推送；若仍失败，提示用户配置 PAT
- 不改动 `.github/workflows/build.yml`（已定稿）

## 关键文件

| 动作 | 文件 |
|---|---|
| 新增 | src/pages/Subscription.tsx、src/components/BottomNav.tsx、src/components/ProGate.tsx、src/components/ui/Toast.tsx、src/pro/gating.ts、src/stores/pro.ts |
| 修改 | src/App.tsx、src/components/Sidebar.tsx、src/pages/Rules.tsx、src/pages/Settings.tsx、src/pages/Connections.tsx、src-tauri/src/ipc/commands.rs、src-tauri/src/core/state.rs、src-tauri/src/core/store.rs、src/api/ipc.ts |

## 验证

1. `npm run tauri dev` 桌面窗口：订阅页增删改查、规则页无订阅卡片、Toast 正常
2. 窗口拖到 <768px 宽：出现底部 Tab、TitleBar/Sidebar 隐藏、页面网格变单列
3. `npm run build`（tsc）+ `cargo check` 无错
4. 门控逻辑单测式手验：浏览器控制台模拟 `isMobile` 返回值（gating.ts 预留 `?forceMobile=1` 调试参数），确认锁态 UI 出现、输入 `PRISM-XXXX-XXXX-XXXX` 格式码解锁、重启保持
