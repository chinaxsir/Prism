//! 每请求上下文：D1 句柄、配置、RS256 签名器（PEM 解析结果按线程缓存）。

use std::cell::RefCell;
use std::sync::Arc;

use worker::{Env, RouteContext};

use crate::config::Config;
use crate::db::Db;
use crate::models::ApiError;
use crate::tokens::{SharedSigner, Signer};

thread_local! {
    // (pem, signer) —— Workers isomorph 线程复用，PEM 不变则避免重复解析 RSA 密钥
    static SIGNER_CACHE: RefCell<Option<(String, SharedSigner)>> = const { RefCell::new(None) };
}

fn get_signer(pem: &str) -> Result<SharedSigner, ApiError> {
    if let Some(found) =
        SIGNER_CACHE.with(|c| c.borrow().as_ref().filter(|(k, _)| k == pem).map(|(_, v)| v.clone()))
    {
        return Ok(found);
    }
    let signer = Arc::new(Signer::from_pem(pem).map_err(ApiError::internal)?);
    SIGNER_CACHE.with(|c| *c.borrow_mut() = Some((pem.to_string(), signer.clone())));
    Ok(signer)
}

pub struct AppCtx {
    pub db: Db,
    pub config: Config,
    pub signer: SharedSigner,
}

impl AppCtx {
    pub fn from_route(ctx: &RouteContext<()>) -> Result<AppCtx, ApiError> {
        Self::from_env(&ctx.env)
    }

    pub fn from_env(env: &Env) -> Result<AppCtx, ApiError> {
        let d1 = env
            .d1("DB")
            .map_err(|e| ApiError::internal(format!("D1 绑定 DB 不可用: {e}")))?;
        let config = Config::from_env(env).map_err(ApiError::internal)?;
        let pem = env
            .secret("LICENSE_RSA_PEM")
            .map(|s| s.to_string())
            .map_err(|_| {
                ApiError::internal(
                    "Secret LICENSE_RSA_PEM 未设置：必须配置与旧服务同一把 PKCS#8 PEM 私钥",
                )
            })?;
        if pem.trim().is_empty() {
            return Err(ApiError::internal("Secret LICENSE_RSA_PEM 为空"));
        }
        let signer = get_signer(&pem)?;
        Ok(AppCtx { db: Db::new(d1), config, signer })
    }
}
