//! 插件系统（预留扩展点）
//!
//! 设计目标：插件可干预流经的流量
//! - 去广告（HTTP 响应改写）
//! - 自定义 DNS 解析
//! - 协议头魔改 / 流量伪装
//! - 脚本化解密
//!
//! 技术路线：
//! 1. 阶段一（当前）：定义 Plugin trait + 事件钩子枚举，
//!    插件编译进主程序（静态注册）。
//! 2. 阶段二：WASM 运行时（wasmtime，见 Cargo.toml feature `wasm-plugins`），
//!    插件以 .wasm 分发，通过 wit 接口调用宿主能力。
//! 3. 阶段三：动态链接库（cdylib）热加载，仅限桌面端。

pub mod hook;

use anyhow::Result;

/// 插件生命周期
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    fn version(&self) -> &str;

    fn on_load(&mut self) -> Result<()> {
        Ok(())
    }
    fn on_unload(&mut self) -> Result<()> {
        Ok(())
    }
}

pub struct PluginRegistry {
    plugins: Vec<Box<dyn Plugin>>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self { plugins: vec![] }
    }

    pub fn register(&mut self, plugin: Box<dyn Plugin>) {
        tracing::info!("register plugin: {}", plugin.name());
        self.plugins.push(plugin);
    }

    /// 按优先级顺序派发事件
    pub fn dispatch(&self, _event: hook::HookEvent) {
        // TODO: 遍历插件，调用对应 hook 处理函数
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}
