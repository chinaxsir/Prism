//! 配置：普通项走 wrangler.toml [vars]，敏感项走 Worker Secrets。

use worker::Env;

#[derive(Clone)]
pub struct Config {
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
    /// Google Play 服务账号 JSON 原文（Secret PRISM_GOOGLE_SA_JSON）
    pub service_account_json: Option<String>,
}

fn var(env: &Env, key: &str) -> Option<String> {
    env.var(key)
        .ok()
        .map(|v| v.to_string())
        .filter(|v| !v.is_empty())
}

fn csv(value: Option<String>) -> Vec<String> {
    value
        .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default()
}

impl Config {
    pub fn from_env(env: &Env) -> Result<Config, String> {
        // admin_key 只接受 Secret
        let admin_key = env
            .secret("PRISM_LICENSE_ADMIN_KEY")
            .map(|s| s.to_string())
            .map_err(|_| "Secret PRISM_LICENSE_ADMIN_KEY 未设置".to_string())?;
        if admin_key.is_empty() {
            return Err("Secret PRISM_LICENSE_ADMIN_KEY 不能为空".into());
        }

        let apple = AppleConfig {
            bundle_id: var(env, "PRISM_APPLE_BUNDLE_ID").unwrap_or_default(),
            subscription_products: csv(var(env, "PRISM_APPLE_SUBSCRIPTION_PRODUCTS")),
            lifetime_products: csv(var(env, "PRISM_APPLE_LIFETIME_PRODUCTS")),
        };
        let google = GoogleConfig {
            package_name: var(env, "PRISM_GOOGLE_PACKAGE").unwrap_or_default(),
            subscription_products: csv(var(env, "PRISM_GOOGLE_SUBSCRIPTION_PRODUCTS")),
            lifetime_products: csv(var(env, "PRISM_GOOGLE_LIFETIME_PRODUCTS")),
            service_account_json: env
                .secret("PRISM_GOOGLE_SA_JSON")
                .ok()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty()),
        };

        Ok(Config { admin_key, apple, google })
    }
}
