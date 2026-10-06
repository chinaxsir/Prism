//! 业务 HTTP 接口：激活码激活/校验、Apple/Google 收据上报、公钥分发。
//! （契约与原 license-server 完全一致：错误码、camelCase、3 天订阅 JWT 宽限。）

use worker::{Request, Response};

use crate::apple;
use crate::context::AppCtx;
use crate::db::now_secs;
use crate::google::PlayClient;
use crate::models::{
    ActivateReq, ApiError, AppleReceiptReq, DeviceInfo, EntitlementDto, GoogleReceiptReq,
    LicenseResponse, VerifyReq,
};
use crate::randutil;
use crate::tokens::{LicenseClaims, verify_license};

async fn parse_json<T: serde::de::DeserializeOwned>(req: &mut Request) -> Result<T, ApiError> {
    req.json::<T>()
        .await
        .map_err(|e| ApiError::bad(format!("请求体解析失败: {e}")))
}

fn validate_device(device: &DeviceInfo) -> Result<(), ApiError> {
    if device.id.trim().is_empty() {
        return Err(ApiError::bad("设备 ID 缺失"));
    }
    Ok(())
}

fn json(resp: &LicenseResponse) -> Result<Response, ApiError> {
    Response::from_json(resp).map_err(|e| ApiError::internal(e.to_string()))
}

pub async fn public_key(app: &AppCtx) -> Result<Response, ApiError> {
    Response::from_json(&serde_json::json!({ "publicKey": app.signer.public_pem() }))
        .map_err(|e| ApiError::internal(e.to_string()))
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
    let now = now_secs();
    let jwt_exp = match expires_at {
        Some(ts) => ts + 3 * 86400, // 订阅 JWT 在到期后保留 3 天（配合客户端宽限期）
        None => now + 10 * 365 * 86400, // 买断 10 年
    };
    LicenseClaims {
        iss: "prism-license".into(),
        iat: now,
        exp: jwt_exp,
        jti: randutil::random_u64().to_string(),
        src: source.into(),
        code: code.map(String::from),
        original_id: original_id.map(String::from),
        dev: device.id.clone(),
        kind: kind.into(),
        exp_at: expires_at,
    }
}

// ---------------- 激活码 ----------------

pub async fn activate(mut req: Request, app: AppCtx) -> Result<Response, ApiError> {
    let body: ActivateReq = parse_json(&mut req).await?;
    validate_device(&body.device)?;
    let code_raw = body.code.trim().to_uppercase();

    // 黑名单设备直接拒绝（在查码之前，避免泄露码状态）
    if app
        .db
        .is_device_blacklisted(&body.device.id)
        .await
        .map_err(ApiError::internal)?
    {
        return Err(ApiError::forbidden("device_banned", "该设备已被封禁"));
    }

    let Some(record) = app
        .db
        .get_code(&code_raw)
        .await
        .map_err(ApiError::internal)?
    else {
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
                let now = now_secs();
                app.db.set_anchor(&code_raw, now).await.map_err(ApiError::internal)?;
                now
            }
        };
        let days = record.duration_days.unwrap_or(365);
        let expires = anchor + days * 86400;
        if expires <= now_secs() {
            return Err(ApiError::forbidden("code_expired", "激活码对应订阅已到期"));
        }
        ("subscription", Some(expires))
    } else {
        ("lifetime", None)
    };

    // 设备数限制：已绑定设备不受限；新设备需未达上限
    let bound_count = app.db.device_count(&code_raw).await.map_err(ApiError::internal)?;
    let is_new = !app
        .db
        .is_device_bound(&code_raw, &body.device.id)
        .await
        .map_err(ApiError::internal)?;
    if is_new && bound_count >= record.max_devices {
        return Err(ApiError::forbidden(
            "device_limit",
            format!(
                "设备数已达上限（{} 台），请在原有设备上解绑后再激活",
                record.max_devices
            ),
        ));
    }
    app.db
        .upsert_code_device(&code_raw, &body.device)
        .await
        .map_err(ApiError::internal)?;

    let claims = build_claims("code", kind, &body.device, Some(&code_raw), None, expires_at);
    let license = app.signer.issue(&claims).map_err(ApiError::internal)?;

    json(&LicenseResponse {
        license: Some(license),
        entitlement: EntitlementDto {
            status: "active".into(),
            kind: kind.into(),
            expires_at,
            source: "code".into(),
            last_verified_at: now_secs(),
        },
    })
}

// ---------------- License 校验 ----------------

pub async fn verify(mut req: Request, app: AppCtx) -> Result<Response, ApiError> {
    let body: VerifyReq = parse_json(&mut req).await?;
    let claims = verify_license(&body.license, app.signer.public_pem(), now_secs())
        .map_err(|_| ApiError::forbidden("license_invalid", "授权凭证无效或已过期，请重新激活"))?;

    // 黑名单设备校验失败
    if app
        .db
        .is_device_blacklisted(&claims.dev)
        .await
        .map_err(ApiError::internal)?
    {
        return Err(ApiError::forbidden("device_banned", "该设备已被封禁"));
    }

    let now = now_secs();

    let (status, kind, expires_at, source): (&str, String, Option<i64>, String) =
        match claims.src.as_str() {
            "code" => {
                let Some(code) = claims.code.as_ref() else {
                    return Err(ApiError::internal("license 缺少 code 声明"));
                };
                let Some(record) = app.db.get_code(code).await.map_err(ApiError::internal)? else {
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
                let Some((_product_id, p_kind, expires)) = app
                    .db
                    .get_store_purchase(&store, original_id)
                    .await
                    .map_err(ApiError::internal)?
                else {
                    return Err(ApiError::forbidden("purchase_missing", "购买记录不存在"));
                };
                let active_now = expires.is_none() || expires > Some(now);
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
        if source == "code" {
            if let Some(code) = claims.code.as_ref() {
                app.db.upsert_code_device(code, &device).await.ok();
            }
        } else if let Some(oid) = claims.original_id.as_ref() {
            app.db.upsert_store_device(&source, oid, &device).await.ok();
        }
    }

    json(&LicenseResponse {
        license: None,
        entitlement: EntitlementDto {
            status: status.to_string(),
            kind,
            expires_at,
            source: source.to_string(),
            last_verified_at: now,
        },
    })
}

// ---------------- 商店收据 ----------------

pub async fn receipt_apple(mut req: Request, app: AppCtx) -> Result<Response, ApiError> {
    let body: AppleReceiptReq = parse_json(&mut req).await?;
    validate_device(&body.device)?;
    if app.config.apple.bundle_id.is_empty() {
        return Err(ApiError::not_implemented("apple_disabled", "服务端未配置 Apple 校验"));
    }

    let normalized = apple::verify(&body.signed_transaction, &app.config.apple)
        .map_err(|e| ApiError::forbidden("receipt_invalid", e))?;
    if normalized.expires_at.is_some_and(|exp| exp <= now_secs()) {
        return Err(ApiError::forbidden("subscription_expired", "订阅已过期，请续费后恢复"));
    }

    app.db
        .upsert_store_purchase(
            "apple",
            &normalized.original_id,
            &normalized.product_id,
            &normalized.kind,
            normalized.expires_at,
            &body.signed_transaction,
        )
        .await
        .map_err(ApiError::internal)?;
    app.db
        .upsert_store_device("apple", &normalized.original_id, &body.device)
        .await
        .map_err(ApiError::internal)?;

    let claims = build_claims(
        "apple",
        &normalized.kind,
        &body.device,
        None,
        Some(&normalized.original_id),
        normalized.expires_at,
    );
    let license = app.signer.issue(&claims).map_err(ApiError::internal)?;

    json(&LicenseResponse {
        license: Some(license),
        entitlement: EntitlementDto {
            status: "active".into(),
            kind: normalized.kind,
            expires_at: normalized.expires_at,
            source: "apple".into(),
            last_verified_at: now_secs(),
        },
    })
}

pub async fn receipt_google(mut req: Request, app: AppCtx) -> Result<Response, ApiError> {
    let body: GoogleReceiptReq = parse_json(&mut req).await?;
    validate_device(&body.device)?;
    let Some(play) = PlayClient::from_config(&app.config.google).map_err(ApiError::internal)?
    else {
        return Err(ApiError::not_implemented(
            "google_disabled",
            "服务端未配置 Google Play 校验（缺少服务账号）",
        ));
    };

    let normalized = play
        .verify(
            &app.config.google.package_name,
            &body.product_id,
            &body.purchase_token,
            body.subscription,
            &app.config.google,
        )
        .await
        .map_err(|e| ApiError::forbidden("receipt_invalid", e))?;
    if normalized.expires_at.is_some_and(|exp| exp <= now_secs()) {
        return Err(ApiError::forbidden("subscription_expired", "订阅已过期，请续费后恢复"));
    }

    app.db
        .upsert_store_purchase(
            "google",
            &normalized.original_id,
            &normalized.product_id,
            &normalized.kind,
            normalized.expires_at,
            &body.purchase_token,
        )
        .await
        .map_err(ApiError::internal)?;
    app.db
        .upsert_store_device("google", &normalized.original_id, &body.device)
        .await
        .map_err(ApiError::internal)?;

    let claims = build_claims(
        "google",
        &normalized.kind,
        &body.device,
        None,
        Some(&normalized.original_id),
        normalized.expires_at,
    );
    let license = app.signer.issue(&claims).map_err(ApiError::internal)?;

    json(&LicenseResponse {
        license: Some(license),
        entitlement: EntitlementDto {
            status: "active".into(),
            kind: normalized.kind,
            expires_at: normalized.expires_at,
            source: "google".into(),
            last_verified_at: now_secs(),
        },
    })
}
