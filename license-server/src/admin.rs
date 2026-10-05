//! 管理能力：HTTP 管理接口（X-Admin-Key）+ CLI 共用的发码/查询逻辑。

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use rand::Rng;
use serde::Deserialize;

use crate::db::{CodeSummary, Db, DeviceRow};
use crate::handlers::AppState;

const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789"; // 去除 I/O/0/1

/// 生成激活码：PRISM-XXXX-XXXX-XXXX
pub fn generate_code() -> String {
    let mut rng = rand::thread_rng();
    let mut block = || -> String {
        (0..4).map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char).collect()
    };
    format!("PRISM-{}-{}-{}", block(), block(), block())
}

/// 校验 X-Admin-Key；返回 Err(响应) 即拒绝
fn check_key(headers: &HeaderMap, expected: &str) -> Result<(), axum::response::Response> {
    let ok = headers
        .get("X-Admin-Key")
        .and_then(|v| v.to_str().ok())
        .map(|v| v == expected)
        .unwrap_or(false);
    if ok {
        Ok(())
    } else {
        Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": { "code": "unauthorized", "message": "管理密钥无效" } })),
        )
            .into_response())
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/codes", get(list).post(issue))
        .route("/admin/codes/{code}/revoke", post(revoke))
        .route("/admin/codes/{code}/devices", get(devices))
        .route("/admin/codes/{code}/devices/{deviceId}", axum::routing::delete(unbind))
}

// ---------------- HTTP handlers ----------------

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct IssueBody {
    kind: Option<String>,
    duration_days: Option<i64>,
    max_devices: Option<i64>,
    note: Option<String>,
    count: Option<i64>,
}

#[derive(Deserialize)]
struct StatusQuery {
    status: Option<String>,
}

async fn issue(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IssueBody>,
) -> Result<Json<serde_json::Value>, axum::response::Response> {
    check_key(&headers, &s.config.admin_key)?;

    let kind = body.kind.unwrap_or_else(|| "lifetime".into());
    if !matches!(kind.as_str(), "lifetime" | "subscription") {
        return Err((StatusCode::BAD_REQUEST, "kind 必须为 lifetime/subscription").into_response());
    }
    let duration = if kind == "subscription" {
        let d = body.duration_days.unwrap_or(365);
        if d <= 0 {
            return Err((StatusCode::BAD_REQUEST, "durationDays 必须大于 0").into_response());
        }
        Some(d)
    } else {
        if body.duration_days.is_some() {
            return Err((StatusCode::BAD_REQUEST, "买断码不能携带 durationDays").into_response());
        }
        None
    };
    let max_devices = body.max_devices.unwrap_or(3).clamp(1, 100);
    let count = body.count.unwrap_or(1).clamp(1, 100);

    let mut codes: Vec<String> = Vec::new();
    for _ in 0..count {
        let code = generate_code();
        s.db.insert_code(&code, &kind, duration, max_devices, body.note.as_deref())
            .await
            .map_err(|e| {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
            })?;
        codes.push(code);
    }
    Ok(Json(serde_json::json!({ "codes": codes })))
}

async fn list(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<StatusQuery>,
) -> Result<Json<Vec<CodeSummary>>, axum::response::Response> {
    check_key(&headers, &s.config.admin_key)?;
    let items = s.db.list_codes(q.status.as_deref()).await.map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
    })?;
    Ok(Json(items))
}

async fn revoke(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(code): Path<String>,
) -> Result<Json<serde_json::Value>, axum::response::Response> {
    check_key(&headers, &s.config.admin_key)?;
    let ok = s.db.revoke_code(&code).await.map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
    })?;
    if !ok {
        return Err((StatusCode::NOT_FOUND, "激活码不存在或已吊销").into_response());
    }
    Ok(Json(serde_json::json!({ "revoked": true })))
}

async fn devices(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(code): Path<String>,
) -> Result<Json<Vec<DeviceRow>>, axum::response::Response> {
    check_key(&headers, &s.config.admin_key)?;
    let rows = s.db.list_devices(&code).await.map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
    })?;
    Ok(Json(rows))
}

async fn unbind(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((code, device_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, axum::response::Response> {
    check_key(&headers, &s.config.admin_key)?;
    let ok = s.db.unbind_device(&code, &device_id).await.map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
    })?;
    if !ok {
        return Err((StatusCode::NOT_FOUND, "设备绑定不存在").into_response());
    }
    Ok(Json(serde_json::json!({ "unbound": true })))
}

// ---------------- CLI ----------------

/// CLI 子命令定义（main.rs 解析）
#[derive(clap::Subcommand)]
pub enum AdminCommand {
    /// 签发激活码
    Issue {
        /// lifetime | subscription
        #[arg(long, default_value = "lifetime")]
        kind: String,
        /// 订阅有效天数（kind=subscription 时，默认 365）
        #[arg(long)]
        duration_days: Option<i64>,
        /// 最多绑定设备数（默认 3）
        #[arg(long, default_value_t = 3)]
        max_devices: i64,
        /// 批量生成数量（默认 1）
        #[arg(long, default_value_t = 1)]
        count: i64,
        /// 备注
        #[arg(long)]
        note: Option<String>,
    },
    /// 列出激活码（可按状态过滤）
    List {
        #[arg(long)]
        status: Option<String>,
    },
    /// 吊销激活码
    Revoke { code: String },
    /// 查看激活码绑定的设备
    Devices { code: String },
    /// 解绑指定设备
    Unbind { code: String, device_id: String },
}

pub async fn run_cli(db: Db, cmd: AdminCommand) -> anyhow::Result<()> {
    match cmd {
        AdminCommand::Issue { kind, duration_days, max_devices, count, note } => {
            if !matches!(kind.as_str(), "lifetime" | "subscription") {
                anyhow::bail!("kind 必须为 lifetime/subscription");
            }
            let duration = if kind == "subscription" {
                Some(duration_days.unwrap_or(365))
            } else {
                None
            };
            for _ in 0..count {
                let code = generate_code();
                db.insert_code(&code, &kind, duration, max_devices, note.as_deref()).await?;
                println!("{code}");
            }
        }
        AdminCommand::List { status } => {
            for c in db.list_codes(status.as_deref()).await? {
                println!(
                    "{}\t{}\t{}\tdevices={}/{}",
                    c.code, c.status, c.kind, c.devices, c.max_devices
                );
            }
        }
        AdminCommand::Revoke { code } => {
            let ok = db.revoke_code(&code.to_uppercase()).await?;
            if !ok {
                anyhow::bail!("激活码不存在或已吊销");
            }
            println!("revoked: {code}");
        }
        AdminCommand::Devices { code } => {
            for d in db.list_devices(&code.to_uppercase()).await? {
                println!(
                    "{}\tname={}\tplatform={}\tlast_seen={}",
                    d.device_id,
                    d.device_name.unwrap_or_default(),
                    d.platform.unwrap_or_default(),
                    d.last_seen
                );
            }
        }
        AdminCommand::Unbind { code, device_id } => {
            let ok = db.unbind_device(&code.to_uppercase(), &device_id).await?;
            if !ok {
                anyhow::bail!("设备绑定不存在");
            }
            println!("unbound: {device_id}");
        }
    }
    Ok(())
}
