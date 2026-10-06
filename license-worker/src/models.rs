//! 共享请求/响应模型与错误类型（与原 license-server HTTP 契约一致）。

use serde::{Deserialize, Serialize};
use worker::Response;

/// 客户端上报的设备信息
#[derive(Deserialize, Clone)]
pub struct DeviceInfo {
    pub id: String,
    pub name: Option<String>,
    pub platform: Option<String>,
    pub app_version: Option<String>,
}

#[derive(Deserialize)]
pub struct ActivateReq {
    pub code: String,
    pub device: DeviceInfo,
}

#[derive(Deserialize)]
pub struct VerifyReq {
    pub license: String,
}

#[derive(Deserialize)]
pub struct AppleReceiptReq {
    pub signed_transaction: String,
    pub device: DeviceInfo,
}

#[derive(Deserialize)]
pub struct GoogleReceiptReq {
    pub purchase_token: String,
    pub product_id: String,
    pub subscription: bool,
    pub device: DeviceInfo,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementDto {
    /// active | expired | revoked（离线宽限由客户端自行标记为 grace）
    pub status: String,
    pub kind: String,
    pub expires_at: Option<i64>,
    pub source: String,
    pub last_verified_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LicenseResponse {
    /// 新签发的 license（verify 时为 null，客户端保留原 license）
    pub license: Option<String>,
    pub entitlement: EntitlementDto,
}

/// 统一错误体：{ error: { code, message } }
pub struct ApiError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn bad(msg: impl Into<String>) -> Self {
        Self { status: 400, code: "bad_request", message: msg.into() }
    }
    pub fn invalid_code() -> Self {
        Self {
            status: 404,
            code: "code_invalid",
            message: "激活码无效，请检查后重试".into(),
        }
    }
    pub fn forbidden(code: &'static str, msg: impl Into<String>) -> Self {
        Self { status: 403, code, message: msg.into() }
    }
    pub fn not_implemented(code: &'static str, msg: impl Into<String>) -> Self {
        Self { status: 501, code, message: msg.into() }
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        Self { status: 500, code: "internal", message: msg.into() }
    }
    pub fn unauthorized() -> Self {
        Self { status: 401, code: "unauthorized", message: "管理密钥无效".into() }
    }
    pub fn plain(status: u16, msg: impl Into<String>) -> Self {
        Self { status, code: "bad_request", message: msg.into() }
    }
}

impl From<ApiError> for Response {
    fn from(e: ApiError) -> Response {
        let body = serde_json::json!({
            "error": { "code": e.code, "message": e.message }
        });
        Response::from_json(&body)
            .unwrap_or_else(|_| Response::error(e.message.clone(), e.status).unwrap())
            .with_status(e.status)
    }
}
