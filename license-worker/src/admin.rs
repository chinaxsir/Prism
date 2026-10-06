//! 管理 HTTP 接口（X-Admin-Key）。CLI 已丢弃：发码/查询走本接口，
//! 直查数据可用 `wrangler d1 execute`。

use serde::Deserialize;
use worker::{Request, Response};

use crate::context::AppCtx;
use crate::models::{ApiError, valid_email};
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
    /// 预绑定邮箱（可选）
    email: Option<String>,
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
    let email = body
        .email
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase);
    if let Some(e) = &email
        && !valid_email(e)
    {
        return Err(ApiError::plain(400, "邮箱格式无效"));
    }

    let mut codes: Vec<String> = Vec::new();
    for _ in 0..count {
        let code = generate_code();
        app.db
            .insert_code(
                &code,
                &kind,
                duration,
                max_devices,
                body.note.as_deref(),
                email.as_deref(),
            )
            .await
            .map_err(ApiError::internal)?;
        codes.push(code);
    }
    Response::from_json(&serde_json::json!({ "codes": codes }))
        .map_err(|e| ApiError::internal(e.to_string()))
}

/// GET /admin/codes?status=active&q=PRISM
pub async fn list(req: Request, app: AppCtx) -> Result<Response, ApiError> {
    check_key(&req, &app.config.admin_key)?;
    let url_str = req.url().map_err(|e| ApiError::internal(e.to_string()))?;
    let url = url::Url::parse(url_str.as_str()).map_err(|e| ApiError::internal(e.to_string()))?;
    let query = |key: &str| {
        url.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let status = query("status");
    let q = query("q");
    let items = app
        .db
        .list_codes(status.as_deref(), q.as_deref())
        .await
        .map_err(ApiError::internal)?;
    Response::from_json(&items).map_err(|e| ApiError::internal(e.to_string()))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct UpdateBody {
    max_devices: Option<i64>,
    duration_days: Option<i64>,
    extend_days: Option<i64>,
    status: Option<String>,
    /// 缺省=不改；null=清空备注
    note: Option<Option<String>>,
    /// 缺省=不改；""=解绑；其余=改绑
    email: Option<String>,
}

/// PATCH /admin/codes/:code —— 编辑设备上限 / 订阅时长 / 延期 / 停用恢复 / 备注
pub async fn update(mut req: Request, code: String, app: AppCtx) -> Result<Response, ApiError> {
    check_key(&req, &app.config.admin_key)?;
    let body: UpdateBody = req
        .json()
        .await
        .map_err(|e| ApiError::bad(format!("请求体解析失败: {e}")))?;

    if let Some(s) = &body.status {
        if !matches!(s.as_str(), "active" | "revoked") {
            return Err(ApiError::plain(400, "status 必须为 active/revoked"));
        }
    }
    if body.max_devices.is_some_and(|m| m <= 0) {
        return Err(ApiError::plain(400, "maxDevices 必须大于 0"));
    }
    if body.duration_days.is_some_and(|d| d <= 0) {
        return Err(ApiError::plain(400, "durationDays 必须大于 0"));
    }
    if body.extend_days.is_some_and(|d| d <= 0) {
        return Err(ApiError::plain(400, "extendDays 必须大于 0"));
    }
    // None=不改；Some("")=解绑；Some(v)=改绑（update_code 内统一小写）
    let email = body.email.as_deref().map(str::trim);
    if let Some(e) = email
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        && !valid_email(&e)
    {
        return Err(ApiError::plain(400, "邮箱格式无效"));
    }
    // 时长类修改仅订阅码允许
    if body.duration_days.is_some() || body.extend_days.is_some() {
        let Some(rec) = app.db.get_code(&code).await.map_err(ApiError::internal)? else {
            return Err(ApiError::plain(404, "激活码不存在"));
        };
        if rec.kind != "subscription" {
            return Err(ApiError::plain(400, "买断码不能修改时长/延期"));
        }
    }

    let ok = app
        .db
        .update_code(
            &code,
            body.max_devices,
            body.duration_days,
            body.extend_days,
            body.status.as_deref(),
            body.note
                .as_ref()
                .map(|o| o.as_deref()),
            email,
        )
        .await
        .map_err(ApiError::internal)?;
    if !ok {
        return Err(ApiError::plain(404, "激活码不存在"));
    }
    Response::from_json(&serde_json::json!({ "updated": true }))
        .map_err(|e| ApiError::internal(e.to_string()))
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

/// DELETE /admin/codes/:code —— 彻底删除激活码及其设备绑定
pub async fn delete_code(req: Request, code: String, app: AppCtx) -> Result<Response, ApiError> {
    check_key(&req, &app.config.admin_key)?;
    let ok = app.db.delete_code(&code).await.map_err(ApiError::internal)?;
    if !ok {
        return Err(ApiError::plain(404, "激活码不存在"));
    }
    Response::from_json(&serde_json::json!({ "deleted": true }))
        .map_err(|e| ApiError::internal(e.to_string()))
}

// ---------------- 黑名单 ----------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BanBody {
    device_id: String,
    reason: Option<String>,
}

/// GET /admin/blacklist
pub async fn blacklist_list(req: Request, app: AppCtx) -> Result<Response, ApiError> {
    check_key(&req, &app.config.admin_key)?;
    let rows = app.db.blacklist_list().await.map_err(ApiError::internal)?;
    Response::from_json(&rows).map_err(|e| ApiError::internal(e.to_string()))
}

/// POST /admin/blacklist {deviceId, reason?}
pub async fn blacklist_add(mut req: Request, app: AppCtx) -> Result<Response, ApiError> {
    check_key(&req, &app.config.admin_key)?;
    let body: BanBody = req
        .json()
        .await
        .map_err(|e| ApiError::bad(format!("请求体解析失败: {e}")))?;
    if body.device_id.trim().is_empty() {
        return Err(ApiError::bad("deviceId 不能为空"));
    }
    app.db
        .blacklist_add(&body.device_id, body.reason.as_deref())
        .await
        .map_err(ApiError::internal)?;
    Response::from_json(&serde_json::json!({ "banned": true }))
        .map_err(|e| ApiError::internal(e.to_string()))
}

/// DELETE /admin/blacklist/:deviceId
pub async fn blacklist_remove(
    req: Request,
    device_id: String,
    app: AppCtx,
) -> Result<Response, ApiError> {
    check_key(&req, &app.config.admin_key)?;
    let ok = app
        .db
        .blacklist_remove(&device_id)
        .await
        .map_err(ApiError::internal)?;
    if !ok {
        return Err(ApiError::plain(404, "黑名单记录不存在"));
    }
    Response::from_json(&serde_json::json!({ "unbanned": true }))
        .map_err(|e| ApiError::internal(e.to_string()))
}

// ---------------- 统计 ----------------

/// GET /admin/stats —— 汇总计数 + 存量/到期/渠道明细 + 最近 14 天每日激活数
pub async fn stats(req: Request, app: AppCtx) -> Result<Response, ApiError> {
    check_key(&req, &app.config.admin_key)?;
    let totals = app.db.stats_totals().await.map_err(ApiError::internal)?;
    let breakdown = app.db.stats_breakdown().await.map_err(ApiError::internal)?;
    let daily = app.db.activations_by_day(14).await.map_err(ApiError::internal)?;
    Response::from_json(&serde_json::json!({
        "totals": totals,
        "breakdown": breakdown,
        "activationsDaily": daily,
    }))
    .map_err(|e| ApiError::internal(e.to_string()))
}
