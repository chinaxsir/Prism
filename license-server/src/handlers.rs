//! HTTP 接口：激活码激活/校验、Apple/Google 收据上报、公钥分发。

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::apple;
use crate::config::Config;
use crate::db::Db;
use crate::google::PlayClient;
use crate::tokens::{LicenseClaims, Signer, verify_license};

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub signer: Arc<Signer>,
    pub config: Arc<Config>,
    pub play: Option<Arc<PlayClient>>,
}

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
struct LicenseResponse {
    /// 新签发的 license（verify 时可能为 null，客户端保留原 license）
    license: Option<String>,
    entitlement: EntitlementDto,
}

/// 统一错误体：{ error: { code, message } }
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn bad(msg: impl Into<String>) -> Self {
        Self { status: StatusCode::BAD_REQUEST, code: "bad_request", message: msg.into() }
    }
    fn invalid_code() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "code_invalid",
            message: "激活码无效，请检查后重试".into(),
        }
    }
    fn forbidden(code: &'static str, msg: impl Into<String>) -> Self {
        Self { status: StatusCode::FORBIDDEN, code, message: msg.into() }
    }
    fn internal(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: msg.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let body = Json(serde_json::json!({
            "error": { "code": self.code, "message": self.message }
        }));
        (self.status, body).into_response()
    }
}

fn validate_device(device: &DeviceInfo) -> Result<(), ApiError> {
    if device.id.trim().is_empty() {
        return Err(ApiError::bad("设备 ID 缺失"));
    }
    Ok(())
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/v1/public-key", get(public_key))
        .route("/api/v1/activate", post(activate))
        .route("/api/v1/verify", post(verify))
        .route("/api/v1/receipt/apple", post(receipt_apple))
        .route("/api/v1/receipt/google", post(receipt_google))
}

async fn healthz() -> &'static str {
    "ok"
}

async fn public_key(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "publicKey": s.signer.public_pem() }))
}

/// 构造 license 声明
fn build_claims(
    source: &str,
    kind: &str,
    device: &DeviceInfo,
    code: Option<&str>,
    original_id: Option<&str>,
    expires_at: Option<i64>,
) -> LicenseClaims {
    let now = crate::db::now_secs();
    let jwt_exp = match expires_at {
        Some(ts) => ts + 3 * 86400, // 订阅 JWT 在到期后保留 3 天（配合客户端宽限期）
        None => now + 10 * 365 * 86400, // 买断 10 年
    };
    LicenseClaims {
        iss: "prism-license".into(),
        iat: now,
        exp: jwt_exp,
        jti: rand::random::<u64>().to_string(),
        src: source.into(),
        code: code.map(String::from),
        original_id: original_id.map(String::from),
        dev: device.id.clone(),
        kind: kind.into(),
        exp_at: expires_at,
    }
}

// ---------------- 激活码 ----------------

async fn activate(
    State(s): State<AppState>,
    Json(req): Json<ActivateReq>,
) -> Result<Json<LicenseResponse>, ApiError> {
    validate_device(&req.device)?;
    let code_raw = req.code.trim().to_uppercase();

    let record = s.db.get_code(&code_raw).await.map_err(|e| ApiError::internal(e.to_string()))?;
    let Some(record) = record else {
        return Err(ApiError::invalid_code());
    };
    if record.status != "active" {
        return Err(ApiError::forbidden("code_revoked", "激活码已被吊销"));
    }

    // 订阅：首次激活锚定时间，后续按 anchor + duration 判定
    let (kind, expires_at) = if record.kind == "subscription" {
        let anchor = match record.anchor {
            Some(a) => a,
            None => {
                let now = crate::db::now_secs();
                s.db.set_anchor(&code_raw, now).await.map_err(|e| ApiError::internal(e.to_string()))?;
                now
            }
        };
        let days = record.duration_days.unwrap_or(365);
        let expires = anchor + days * 86400;
        if expires <= crate::db::now_secs() {
            return Err(ApiError::forbidden("code_expired", "激活码对应订阅已到期"));
        }
        ("subscription", Some(expires))
    } else {
        ("lifetime", None)
    };

    // 设备数限制：已绑定设备不受限；新设备需未达上限
    let bound_count = s.db.device_count(&code_raw).await.map_err(|e| ApiError::internal(e.to_string()))?;
    let is_new = !s.db.is_device_bound(&code_raw, &req.device.id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if is_new && bound_count >= record.max_devices {
        return Err(ApiError::forbidden(
            "device_limit",
            format!("设备数已达上限（{} 台），请在原有设备上解绑后再激活", record.max_devices),
        ));
    }
    s.db.upsert_code_device(&code_raw, &req.device)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let claims = build_claims(
        "code", kind, &req.device, Some(&code_raw), None, expires_at,
    );
    let license = s
        .signer
        .issue(&claims)
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(LicenseResponse {
        license: Some(license),
        entitlement: EntitlementDto {
            status: "active".into(),
            kind: kind.into(),
            expires_at,
            source: "code".into(),
            last_verified_at: crate::db::now_secs(),
        },
    }))
}

// ---------------- License 校验 ----------------

async fn verify(
    State(s): State<AppState>,
    Json(req): Json<VerifyReq>,
) -> Result<Json<LicenseResponse>, ApiError> {
    let claims = verify_license(&req.license, s.signer.public_pem())
        .map_err(|_| ApiError::forbidden("license_invalid", "授权凭证无效或已过期，请重新激活"))?;

    let now = crate::db::now_secs();

    let (status, kind, expires_at, source): (&str, String, Option<i64>, String) =
        match claims.src.as_str() {
        "code" => {
            let Some(code) = claims.code.as_ref() else {
                return Err(ApiError::internal("license 缺少 code 声明"));
            };
            let record = s.db.get_code(code).await.map_err(|e| ApiError::internal(e.to_string()))?;
            let Some(record) = record else {
                return Err(ApiError::forbidden("code_invalid", "激活码已不存在"));
            };
            if record.status != "active" {
                ("revoked", record.kind, None, "code".to_string())
            } else if record.kind == "subscription" {
                let anchor = record.anchor.unwrap_or(now);
                let expires = anchor + record.duration_days.unwrap_or(365) * 86400;
                let active_now = expires > now;
                (
                    if active_now { "active" } else { "expired" },
                    "subscription".to_string(),
                    Some(expires),
                    "code".to_string(),
                )
            } else {
                ("active", "lifetime".to_string(), None, "code".to_string())
            }
        }
        "apple" | "google" => {
            let store = claims.src.clone();
            let Some(original_id) = claims.original_id.as_ref() else {
                return Err(ApiError::internal("license 缺少 original_id"));
            };
            let purchase = s
                .db
                .get_store_purchase(&store, original_id)
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?;
            let Some((product_id, p_kind, expires)) = purchase else {
                return Err(ApiError::forbidden("purchase_missing", "购买记录不存在"));
            };
            let active_now = expires.is_none() || expires > Some(now);
            let _ = product_id;
            (
                if active_now { "active" } else { "expired" },
                p_kind,
                expires,
                store,
            )
        }
        other => return Err(ApiError::internal(format!("未知授权来源: {other}"))),
    };

    let is_active = status == "active";
    // 刷新设备活跃时间（失败忽略）
    if is_active {
        let device = DeviceInfo {
            id: claims.dev.clone(),
            name: None,
            platform: None,
            app_version: None,
        };
        if source == "code"
            && let Some(code) = claims.code.as_ref()
        {
            s.db.upsert_code_device(code, &device).await.ok();
        } else if let Some(oid) = claims.original_id.as_ref() {
            s.db.upsert_store_device(&source, oid, &device).await.ok();
        }
    }

    Ok(Json(LicenseResponse {
        license: None,
        entitlement: EntitlementDto {
            status: status.to_string(),
            kind,
            expires_at,
            source: source.to_string(),
            last_verified_at: now,
        },
    }))
}

// ---------------- 商店收据 ----------------

async fn receipt_apple(
    State(s): State<AppState>,
    Json(req): Json<AppleReceiptReq>,
) -> Result<Json<LicenseResponse>, ApiError> {
    validate_device(&req.device)?;
    if s.config.apple.bundle_id.is_empty() {
        return Err(ApiError {
            status: StatusCode::NOT_IMPLEMENTED,
            code: "apple_disabled",
            message: "服务端未配置 Apple 校验".into(),
        });
    }

    let normalized = apple::verify(&req.signed_transaction, &s.config.apple)
        .map_err(|e| ApiError::forbidden("receipt_invalid", e.to_string()))?;
    if let Some(exp) = normalized.expires_at
        && exp <= crate::db::now_secs()
    {
        return Err(ApiError::forbidden("subscription_expired", "订阅已过期，请续费后恢复"));
    }

    s.db.upsert_store_purchase(
        "apple",
        &normalized.original_id,
        &normalized.product_id,
        &normalized.kind,
        normalized.expires_at,
        &req.signed_transaction,
    )
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    s.db.upsert_store_device("apple", &normalized.original_id, &req.device)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let claims = build_claims(
        "apple",
        &normalized.kind,
        &req.device,
        None,
        Some(&normalized.original_id),
        normalized.expires_at,
    );
    let license = s
        .signer
        .issue(&claims)
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(LicenseResponse {
        license: Some(license),
        entitlement: EntitlementDto {
            status: "active".into(),
            kind: normalized.kind,
            expires_at: normalized.expires_at,
            source: "apple".into(),
            last_verified_at: crate::db::now_secs(),
        },
    }))
}

async fn receipt_google(
    State(s): State<AppState>,
    Json(req): Json<GoogleReceiptReq>,
) -> Result<Json<LicenseResponse>, ApiError> {
    validate_device(&req.device)?;
    let Some(play) = &s.play else {
        return Err(ApiError {
            status: StatusCode::NOT_IMPLEMENTED,
            code: "google_disabled",
            message: "服务端未配置 Google Play 校验（缺少服务账号）".into(),
        });
    };

    let normalized = play
        .verify(
            &s.config.google.package_name,
            &req.product_id,
            &req.purchase_token,
            req.subscription,
            &s.config.google,
        )
        .await
        .map_err(|e| ApiError::forbidden("receipt_invalid", e.to_string()))?;
    if let Some(exp) = normalized.expires_at
        && exp <= crate::db::now_secs()
    {
        return Err(ApiError::forbidden("subscription_expired", "订阅已过期，请续费后恢复"));
    }

    s.db.upsert_store_purchase(
        "google",
        &normalized.original_id,
        &normalized.product_id,
        &normalized.kind,
        normalized.expires_at,
        &req.purchase_token,
    )
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    s.db.upsert_store_device("google", &normalized.original_id, &req.device)
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;

    let claims = build_claims(
        "google",
        &normalized.kind,
        &req.device,
        None,
        Some(&normalized.original_id),
        normalized.expires_at,
    );
    let license = s
        .signer
        .issue(&claims)
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(LicenseResponse {
        license: Some(license),
        entitlement: EntitlementDto {
            status: "active".into(),
            kind: normalized.kind,
            expires_at: normalized.expires_at,
            source: "google".into(),
            last_verified_at: crate::db::now_secs(),
        },
    }))
}
