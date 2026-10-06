//! Prism 授权服务（Cloudflare Workers + D1）入口：手动路由。
//!
//! 业务路由：
//!   GET  /healthz
//!   GET  /api/v1/public-key
//!   POST /api/v1/activate
//!   POST /api/v1/verify
//!   POST /api/v1/receipt/apple
//!   POST /api/v1/receipt/google
//! 管理路由（X-Admin-Key）：
//!   GET/POST /admin/codes
//!   PATCH    /admin/codes/:code
//!   DELETE   /admin/codes/:code
//!   POST     /admin/codes/:code/revoke
//!   GET      /admin/codes/:code/devices
//!   DELETE   /admin/codes/:code/devices/:deviceId
//!   GET/POST /admin/blacklist
//!   DELETE   /admin/blacklist/:deviceId
//!   GET      /admin/stats
//! 管理后台页面：
//!   GET      /admin（内嵌 HTML，密钥存 localStorage）

mod admin;
mod apple;
mod config;
mod context;
mod dashboard;
mod db;
mod google;
mod handlers;
mod models;
mod randutil;
mod tokens;

use worker::{Context, Env, Request, Response, Result as WResult, Router};

use crate::context::AppCtx;

/// 统一把业务错误转成 HTTP 响应（路由层不抛错）
fn to_response(r: Result<Response, models::ApiError>) -> WResult<Response> {
    match r {
        Ok(resp) => Ok(resp),
        Err(e) => Ok(e.into()),
    }
}

#[worker::event(fetch)]
async fn main(req: Request, env: Env, _ctx: Context) -> WResult<Response> {
    Router::new()
        .get_async("/healthz", |_req, _ctx| async { Response::ok("ok") })
        .get_async("/api/v1/public-key", |_req, ctx| async move {
            to_response(match AppCtx::from_route(&ctx) {
                Ok(app) => handlers::public_key(&app).await,
                Err(e) => Err(e),
            })
        })
        .post_async("/api/v1/activate", |req, ctx| async move {
            to_response(match AppCtx::from_route(&ctx) {
                Ok(app) => handlers::activate(req, app).await,
                Err(e) => Err(e),
            })
        })
        .post_async("/api/v1/verify", |req, ctx| async move {
            to_response(match AppCtx::from_route(&ctx) {
                Ok(app) => handlers::verify(req, app).await,
                Err(e) => Err(e),
            })
        })
        .post_async("/api/v1/receipt/apple", |req, ctx| async move {
            to_response(match AppCtx::from_route(&ctx) {
                Ok(app) => handlers::receipt_apple(req, app).await,
                Err(e) => Err(e),
            })
        })
        .post_async("/api/v1/receipt/google", |req, ctx| async move {
            to_response(match AppCtx::from_route(&ctx) {
                Ok(app) => handlers::receipt_google(req, app).await,
                Err(e) => Err(e),
            })
        })
        .get_async("/admin/codes", |req, ctx| async move {
            to_response(match AppCtx::from_route(&ctx) {
                Ok(app) => admin::list(req, app).await,
                Err(e) => Err(e),
            })
        })
        .post_async("/admin/codes", |req, ctx| async move {
            to_response(match AppCtx::from_route(&ctx) {
                Ok(app) => admin::issue(req, app).await,
                Err(e) => Err(e),
            })
        })
        .post_async("/admin/codes/:code/revoke", |req, ctx| async move {
            let code = ctx.param("code").cloned().unwrap_or_default();
            to_response(match AppCtx::from_route(&ctx) {
                Ok(app) => admin::revoke(req, code, app).await,
                Err(e) => Err(e),
            })
        })
        .patch_async("/admin/codes/:code", |req, ctx| async move {
            let code = ctx.param("code").cloned().unwrap_or_default();
            to_response(match AppCtx::from_route(&ctx) {
                Ok(app) => admin::update(req, code, app).await,
                Err(e) => Err(e),
            })
        })
        .get_async("/admin/codes/:code/devices", |req, ctx| async move {
            let code = ctx.param("code").cloned().unwrap_or_default();
            to_response(match AppCtx::from_route(&ctx) {
                Ok(app) => admin::devices(req, code, app).await,
                Err(e) => Err(e),
            })
        })
        .delete_async(
            "/admin/codes/:code/devices/:deviceId",
            |req, ctx| async move {
                let code = ctx.param("code").cloned().unwrap_or_default();
                let device_id = ctx.param("deviceId").cloned().unwrap_or_default();
                to_response(match AppCtx::from_route(&ctx) {
                    Ok(app) => admin::unbind(req, code, device_id, app).await,
                    Err(e) => Err(e),
                })
            },
        )
        .delete_async("/admin/codes/:code", |req, ctx| async move {
            let code = ctx.param("code").cloned().unwrap_or_default();
            to_response(match AppCtx::from_route(&ctx) {
                Ok(app) => admin::delete_code(req, code, app).await,
                Err(e) => Err(e),
            })
        })
        .get_async("/admin/blacklist", |req, ctx| async move {
            to_response(match AppCtx::from_route(&ctx) {
                Ok(app) => admin::blacklist_list(req, app).await,
                Err(e) => Err(e),
            })
        })
        .post_async("/admin/blacklist", |req, ctx| async move {
            to_response(match AppCtx::from_route(&ctx) {
                Ok(app) => admin::blacklist_add(req, app).await,
                Err(e) => Err(e),
            })
        })
        .delete_async("/admin/blacklist/:deviceId", |req, ctx| async move {
            let device_id = ctx.param("deviceId").cloned().unwrap_or_default();
            to_response(match AppCtx::from_route(&ctx) {
                Ok(app) => admin::blacklist_remove(req, device_id, app).await,
                Err(e) => Err(e),
            })
        })
        .get_async("/admin/stats", |req, ctx| async move {
            to_response(match AppCtx::from_route(&ctx) {
                Ok(app) => admin::stats(req, app).await,
                Err(e) => Err(e),
            })
        })
        .get_async("/admin", |_req, _ctx| async {
            Response::from_html(crate::dashboard::PAGE)
        })
        .run(req, env)
        .await
}
