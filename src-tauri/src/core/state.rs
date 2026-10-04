//! 应用核心状态机与用户设置
//!
//! 状态流转：
//!   Stopped ──start()──▶ Starting ──init ok──▶ Running
//!      ▲                    │                    │
//!      │                    └──fail──▶ Error ◀───┘
//!      └──────────stop()───────────────┘

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CoreStatus {
    Stopped,
    Starting,
    Running,
    Stopping,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunMode {
    /// 仅系统代理（HTTP/SOCKS 入站）
    SystemProxy,
    /// TUN 全局接管
    Tun,
    /// 仅规则内分流，不设置系统代理
    RuleOnly,
}

/// 用户设置（可由设置页修改，持久化到 settings.json）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSettings {
    /// mixed 混合入站端口（HTTP + SOCKS）
    pub mixed_port: u16,
    /// 允许局域网连接
    pub allow_lan: bool,
    /// 启动时设置系统代理（SystemProxy 模式下生效）
    pub system_proxy: bool,
    /// 开机自启动
    pub auto_start: bool,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            mixed_port: 2080,
            allow_lan: false,
            system_proxy: true,
            auto_start: false,
        }
    }
}

/// 跨 IPC 共享的全局状态
pub struct AppState {
    /// 应用数据目录（config.json / profile / 内核 / 持久化配置）
    pub data_dir: PathBuf,
    pub status: RwLock<CoreStatus>,
    pub mode: RwLock<RunMode>,
    pub settings: RwLock<UserSettings>,
    /// 本次启动时间（用于计算 uptime）
    pub started_at: RwLock<Option<Instant>>,
    pub kernel_handle: RwLock<Option<Arc<super::kernel::KernelHandle>>>,
    /// TUN 生命周期守卫（仅 TUN 模式下存在）
    pub tun_guard: RwLock<Option<super::tun::TunGuard>>,
    /// 当前内核 clash_api 实际地址（启动前为首选值）
    pub api_addr: RwLock<String>,
    /// 当前内核 clash_api 鉴权密钥
    pub api_secret: RwLock<String>,
    /// 已探测到的内核版本（用于设置页展示）
    pub kernel_version: RwLock<Option<String>>,
}

impl AppState {
    /// 创建状态并从 data_dir/settings.json 恢复用户设置
    pub fn new(data_dir: PathBuf) -> Self {
        let settings = super::store::load_settings(&data_dir);
        Self {
            data_dir,
            status: RwLock::new(CoreStatus::Stopped),
            mode: RwLock::new(RunMode::SystemProxy),
            settings: RwLock::new(settings),
            started_at: RwLock::new(None),
            kernel_handle: RwLock::new(None),
            tun_guard: RwLock::new(None),
            api_addr: RwLock::new(super::kernel::PREFERRED_API_ADDR.to_string()),
            api_secret: RwLock::new(String::new()),
            kernel_version: RwLock::new(None),
        }
    }

    /// 构造与当前内核对话的 API 客户端
    pub fn kernel_api(&self) -> super::kernel::KernelApi {
        super::kernel::KernelApi::new(&self.api_addr.read(), &self.api_secret.read())
    }

    /// 保存设置到内存 + settings.json
    pub fn save_settings(&self, next: UserSettings) -> anyhow::Result<()> {
        super::store::save_settings(&self.data_dir, &next)?;
        *self.settings.write() = next;
        Ok(())
    }

    /// 内核工作目录（config.json / profile.yaml / kernel.log 所在）
    pub fn work_dir(&self) -> PathBuf {
        self.data_dir.join("kernel")
    }

    /// 带合法性校验的状态流转
    pub fn transition(&self, next: CoreStatus) -> anyhow::Result<()> {
        let mut status = self.status.write();
        let current = *status;
        let valid = matches!(
            (current, next),
            (CoreStatus::Stopped, CoreStatus::Starting)
                | (CoreStatus::Error, CoreStatus::Starting)
                | (CoreStatus::Starting, CoreStatus::Running)
                | (CoreStatus::Starting, CoreStatus::Error)
                | (CoreStatus::Running, CoreStatus::Stopping)
                | (CoreStatus::Running, CoreStatus::Error)
                | (CoreStatus::Stopping, CoreStatus::Stopped)
                | (CoreStatus::Stopping, CoreStatus::Error)
                | (CoreStatus::Error, CoreStatus::Stopped)
        );
        if !valid {
            anyhow::bail!("invalid state transition: {:?} -> {:?}", current, next);
        }
        *status = next;
        tracing::info!("core state: {:?} -> {:?}", current, next);
        Ok(())
    }
}

/// 供 IPC 层使用：判断端口是否可绑定
pub fn is_port_free(host: &str, port: u16) -> bool {
    std::net::TcpListener::bind((host, port)).is_ok()
}

/// 选择 clash_api 监听地址：优先 9090，被占用则让系统分配随机端口
pub fn pick_api_addr() -> String {
    if is_port_free("127.0.0.1", 9090) {
        return super::kernel::PREFERRED_API_ADDR.to_string();
    }
    match std::net::TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => listener
            .local_addr()
            .map(|a| format!("127.0.0.1:{}", a.port()))
            .unwrap_or_else(|_| "127.0.0.1:0".into()),
        Err(e) => {
            tracing::error!("failed to allocate api port: {e}");
            super::kernel::PREFERRED_API_ADDR.to_string()
        }
    }
}
