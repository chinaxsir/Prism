# Prism 核心架构设计

## 1. 分层架构

```
┌─────────────────────────────────────────────┐
│  React UI (src/)                            │
│  仪表盘 / 节点 / 规则 / 连接日志 / 设置       │
└──────────────┬──────────────────────────────┘
               │ Tauri IPC (invoke / event)
┌──────────────▼──────────────────────────────┐
│  Rust Shell (src-tauri/src/)                │
│  ├─ ipc/       命令层，参数校验               │
│  ├─ core/      内核生命周期 & 状态机          │
│  ├─ platform/  系统代理 / 提权 / 自启动       │
│  └─ plugins/   插件注册与钩子派发             │
└──────────────┬──────────────────────────────┘
               │ 本地 HTTP API (127.0.0.1:9090)
               │ + WebSocket (traffic/connections)
┌──────────────▼──────────────────────────────┐
│  Go Kernel (sing-box sidecar 二进制)         │
│  Inbound (socks/http/tun) → Router → Outbound│
└─────────────────────────────────────────────┘
```

## 2. 核心状态机伪代码

```text
enum CoreStatus { Stopped, Starting, Running, Stopping, Error }
enum RunMode    { SystemProxy, Tun, RuleOnly }

function startCore():
    state.transition(Stopped → Starting)
    config = buildRuntimeConfig(userProfile)     # 订阅+规则 → sing-box JSON/YAML
    kernel = KernelHandle.spawn(config)          # 启动 sidecar 进程
    waitUntil(kernel.api.isReady(), timeout=5s)  # 轮询 /version

    match state.mode:
        SystemProxy → platform.setSystemProxy(127.0.0.1, mixedPort)
        Tun         → require(hasElevatedPrivilege() or requestElevation())
                      kernel.enableTunInbound()   # 由内核处理数据面
        RuleOnly    → noop

    subscribeWebSocket("/traffic",     → emit Tauri event "traffic://tick")
    subscribeWebSocket("/connections", → emit Tauri event "connections://tick")
    state.transition(Starting → Running)

function stopCore():
    state.transition(Running → Stopping)
    kernel.shutdown(graceful=true)
    platform.clearSystemProxy()                  # 必须兜底，防代理残留
    state.transition(Stopping → Stopped)

onProcessExit:                                   # 崩溃兜底
    if state.status != Stopped: platform.clearSystemProxy()
```

## 3. 规则分流数据流

```text
用户订阅 (YAML/JSON)
   │
   ▼
Profile Parser (Rust)  ──解析节点/策略组/规则引用──┐
   │                                                │
   ▼                                                │
Runtime Config Builder                             │
   ├─ outbounds: 节点 + 策略组                       │
   │     (selector / url-test / fallback /          │
   │      load-balance 映射为内核对应结构)            │
   ├─ route.rules: 规则数组 (有序，先匹配优先)        │
   │     DOMAIN / DOMAIN-SUFFIX / DOMAIN-KEYWORD    │
   │     IP-CIDR / GEOIP / GEOSITE / PROCESS-NAME   │
   ├─ route.rule_set: 远程/本地规则集引用            │
   │     (内核自动下载并缓存到 work_dir/rulesets)    │
   └─ dns: fake-ip / 分流 DNS                       │
   │                                                │
   ▼                                                │
写入 work_dir/config.yaml ──▶ Kernel 热重载 ◀──────┘
```

策略组映射（配置层 → 内核）：

| 用户层类型   | sing-box outbound 类型            |
|--------------|-----------------------------------|
| Selector     | `selector`                        |
| URL-Test     | `urltest` (interval + tolerance)  |
| Fallback     | `urltest` (tolerance=0) 或自实现  |
| Load-Balance | `loadbalance` (sing-box 1.12+)    |

## 4. 插件钩子点（预留）

```text
                 ┌── PreDnsResolve      (改/拒 DNS 查询)
DNS 查询 ────────┤
                 └── (fallback 系统解析)

                 ┌── PreConnect         (改写目标/强制出站)
新连接 ──────────┤── 内核规则匹配
                 └── PostRuleMatch      (覆盖匹配结果)

HTTP 流量 ───────┬── HttpRequest        (请求头注入/广告拦截)
  (经 MITM 或插件隧道)
                 └── HttpResponse       (响应改写)
```

阶段一：Rust trait 静态注册（见 src-tauri/src/plugins/）。
阶段二：WASM 插件（wasmtime），通过 wit 定义宿主接口。
阶段三：桌面端 cdylib 热加载。

## 5. 前端实时数据通道

| 数据       | 通道                                    | 频率     |
|------------|-----------------------------------------|----------|
| 上下行速率 | Kernel WS /traffic → Tauri event → 前端 | 1s       |
| 连接日志   | Kernel WS /connections → 增量 diff 推送 | 1s       |
| 内存占用   | Kernel WS /memory                       | 2s       |
| 测速结果   | invoke url_test（一次性请求）           | 手动触发 |

前端用 zustand 存储，Recharts 绘制速率曲线（环形缓冲保留最近 60 个点）。

## 6. 权限与平台矩阵

| 平台    | 系统代理        | TUN                          | 打包产物   |
|---------|----------------|------------------------------|------------|
| Windows | 注册表 HKCU     | wintun.dll + 管理员           | .exe/.msi  |
| macOS   | networksetup   | utun + root（或 NE 扩展）     | .dmg       |
| Android | VpnService     | VpnService API                | .apk       |
| iOS     | —              | Network Extension（需 Xcode entitlement） | .ipa |
```
