//! 服务端配置：全部来自环境变量（部署时通过 systemd / Docker env 注入）。

use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(Clone)]
pub struct Config {
    pub bind: String,
    pub db_path: PathBuf,
    pub key_path: PathBuf,
    /// 管理 CLI / 管理 HTTP 接口共享密钥（必填，缺失则拒绝启动）
    pub admin_key: String,
    pub apple: AppleConfig,
    pub google: GoogleConfig,
}

#[derive(Clone, Default)]
pub struct AppleConfig {
    pub bundle_id: String,
    pub subscription_products: Vec<String>,
    pub lifetime_products: Vec<String>,
}

#[derive(Clone, Default)]
pub struct GoogleConfig {
    pub package_name: String,
    pub subscription_products: Vec<String>,
    pub lifetime_products: Vec<String>,
    /// Google Play 服务账号 JSON 文件路径（AndroidPublisher API 鉴权）
    pub service_account_json: Option<PathBuf>,
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn csv(value: Option<String>) -> Vec<String> {
    value
        .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default()
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let bind = env("PRISM_LICENSE_BIND").unwrap_or_else(|| "0.0.0.0:8787".into());
        let db_path = PathBuf::from(
            env("PRISM_LICENSE_DB").unwrap_or_else(|| "data/license.db".into()),
        );
        let key_path = PathBuf::from(
            env("PRISM_LICENSE_KEY").unwrap_or_else(|| "data/keys/license_rsa.pem".into()),
        );
        let admin_key = env("PRISM_LICENSE_ADMIN_KEY")
            .context("PRISM_LICENSE_ADMIN_KEY 必须设置（管理接口密钥，建议 32+ 随机字符）")?;

        let apple = AppleConfig {
            bundle_id: env("PRISM_APPLE_BUNDLE_ID").unwrap_or_default(),
            subscription_products: csv(env("PRISM_APPLE_SUBSCRIPTION_PRODUCTS")),
            lifetime_products: csv(env("PRISM_APPLE_LIFETIME_PRODUCTS")),
        };
        let google = GoogleConfig {
            package_name: env("PRISM_GOOGLE_PACKAGE").unwrap_or_default(),
            subscription_products: csv(env("PRISM_GOOGLE_SUBSCRIPTION_PRODUCTS")),
            lifetime_products: csv(env("PRISM_GOOGLE_LIFETIME_PRODUCTS")),
            service_account_json: env("PRISM_GOOGLE_SA_JSON").map(PathBuf::from),
        };

        Ok(Config { bind, db_path, key_path, admin_key, apple, google })
    }
}
