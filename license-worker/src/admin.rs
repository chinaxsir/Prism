//! 管理 HTTP 接口（X-Admin-Key）。CLI 已丢弃：发码/查询走本接口，
//! 直查数据可用 `wrangler d1 execute`。

use serde::Deserialize;
use worker::{Request, Response};

use crate::context::AppCtx;
use crate::models::ApiError;
use crate::randutil;

const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789"; // 去除 I/O/0/1

/// 生成激活码：PRISM-XXXX-XXXX-XXXX
pub fn generate_code() -> String {
    let bytes = randutil::random_bytes(12);
    let block = |start: usize| -> String {
        (0..4)
            .map(|i| ALPHABET[(bytes[start + i] as usize) % ALPHABET.len()] as char)
            .collect()
    };
    format!("PRISM-{}-{}-{}", block(0), block(4), block(8))
}

fn check_key(req: &Request, expected: &str) -> Result<(), ApiError> {
    let ok = req
        .headers()
        .get("X-Admin-Key")
        .ok()
        .flatten()
        .map(|v| v == expected)
        .unwrap_or(false);
    if ok {
        Ok(())
    } else {
        Err(ApiError::unauthorized())
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct IssueBody {
    kind: Option<String>,
    duration_days: Option<i64>,
    max_devices: Option<i64>,
    note: Option<String>,
    count: Option<i64>,
}

/// POST /admin/codes
pub async fn issue(mut req: Request, app: AppCtx) -> Result<Response, ApiError> {
    check_key(&req, &app.config.admin_key)?;
    let body: IssueBody = req
        .json()
        .await
        .map_err(|e| ApiError::bad(format!("请求体解析失败: {e}")))?;

    let kind = body.kind.unwrap_or_else(|| "lifetime".into());
    if !matches!(kind.as_str(), "lifetime" | "subscription") {
        return Err(ApiError::plain(400, "kind 必须为 lifetime/subscription"));
    }
    let duration = if kind == "subscription" {
        let d = body.duration_days.unwrap_or(365);
        if d <= 0 {
            return Err(ApiError::plain(400, "durationDays 必须大于 0"));
        }
        Some(d)
    } else {
        if body.duration_days.is_some() {
            return Err(ApiError::plain(400, "买断码不能携带 durationDays"));
        }
        None
    };
    let max_devices = body.max_devices.unwrap_or(3).clamp(1, 100);
    let count = body.count.unwrap_or(1).clamp(1, 100);

    let mut codes: Vec<String> = Vec::new();
    for _ in 0..count {
        let code = generate_code();
        app.db
            .insert_code(&code, &kind, duration, max_devices, body.note.as_deref())
            .await
            .map_err(ApiError::internal)?;
        codes.push(code);
    }
    Response::from_json(&serde_json::json!({ "codes": codes }))
        .map_err(|e| ApiError::internal(e.to_string()))
}

/// GET /admin/codes?status=active
pub async fn list(req: Request, app: AppCtx) -> Result<Response, ApiError> {
    check_key(&req, &app.config.admin_key)?;
    let url_str = req.url().map_err(|e| ApiError::internal(e.to_string()))?;
    let url = url::Url::parse(url_str.as_str()).map_err(|e| ApiError::internal(e.to_string()))?;
    let status = url
        .query_pairs()
        .find(|(k, _)| k == "status")
        .map(|(_, v)| v.trim().to_string())
        .filter(|s| !s.is_empty());
    let items = app
        .db
        .list_codes(status.as_deref())
        .await
        .map_err(ApiError::internal)?;
    Response::from_json(&items).map_err(|e| ApiError::internal(e.to_string()))
}

/// POST /admin/codes/:code/revoke
pub async fn revoke(req: Request, code: String, app: AppCtx) -> Result<Response, ApiError> {
    check_key(&req, &app.config.admin_key)?;
    let ok = app.db.revoke_code(&code).await.map_err(ApiError::internal)?;
    if !ok {
        return Err(ApiError::plain(404, "激活码不存在或已吊销"));
    }
    Response::from_json(&serde_json::json!({ "revoked": true }))
        .map_err(|e| ApiError::internal(e.to_string()))
}

/// GET /admin/codes/:code/devices
pub async fn devices(req: Request, code: String, app: AppCtx) -> Result<Response, ApiError> {
    check_key(&req, &app.config.admin_key)?;
    let rows = app.db.list_devices(&code).await.map_err(ApiError::internal)?;
    Response::from_json(&rows).map_err(|e| ApiError::internal(e.to_string()))
}

/// DELETE /admin/codes/:code/devices/:deviceId
pub async fn unbind(
    req: Request,
    code: String,
    device_id: String,
    app: AppCtx,
) -> Result<Response, ApiError> {
    check_key(&req, &app.config.admin_key)?;
    let ok = app
        .db
        .unbind_device(&code, &device_id)
        .await
        .map_err(ApiError::internal)?;
    if !ok {
        return Err(ApiError::plain(404, "设备绑定不存在"));
    }
    Response::from_json(&serde_json::json!({ "unbound": true }))
        .map_err(|e| ApiError::internal(e.to_string()))
}
