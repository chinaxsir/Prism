//! Pro 授权客户端：设备标识、HTTP 激活与状态查询、本地缓存。
//!
//! 协议（无 token 鉴权；服务端部署于 Cloudflare 后，所有请求带 `X-Request-Source: prism-app` 头）：
//! - 激活：POST /api/license/activate，body {license_key, email, device_id, device_name}
//!   200 → {ok, device_id, plan: "free"|"pro"|"team", max_seats?}；401 无效/停用；409 设备已绑定等冲突；429 → {retry_after: 秒}
//! - 状态：GET /api/license/status?license_key=...
//!   200 → {valid, plan, status, expires_at(ISO8601|null), max_seats?, used_seats?}
//!
//! 授权有效性策略：仅服务端确认 active 时解锁 Pro；断网/服务不可达立即
//! 锁定（status=offline），联网后由后台周期校验自动恢复。不做离线宽限。
//!
//! 本地仅缓存 license_key 与最近一次授权快照（信任 HTTPS 传输，不再做本地签名验证）。

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::state::UserSettings;

/// 编译期固定授权服务地址（构建时可用 PRISM_LICENSE_URL 覆盖；App 内置，用户无需输入）
const DEFAULT_SERVER: &str = match option_env!("PRISM_LICENSE_URL") {
    Some(v) => v,
    None => "https://askcn.de",
};

/// 新协议要求的请求来源标识头
const REQUEST_SOURCE: &str = "prism-app";

// ---------------- 对外类型 ----------------

/// 授权状态快照（前端 gating 直接消费；字段名 camelCase）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementStatus {
    /// inactive | active | offline | expired | revoked
    pub status: String,
    /// lifetime | subscription
    pub kind: Option<String>,
    pub expires_at: Option<i64>,
    /// code | apple | google
    pub source: Option<String>,
    pub last_verified_at: i64,
    /// 服务端方案：free | pro | team（与 /api/license/status 响应一致）
    pub plan: Option<String>,
    /// 授权席位上限（服务端返回时透传）
    pub max_seats: Option<u32>,
    /// 已用席位数（服务端返回时透传）
    pub used_seats: Option<u32>,
}

impl EntitlementStatus {
    pub fn inactive() -> Self {
        Self {
            status: "inactive".into(),
            kind: None,
            expires_at: None,
            source: None,
            last_verified_at: 0,
            plan: None,
            max_seats: None,
            used_seats: None,
        }
    }

    /// 是否可使用 Pro：仅服务端联网校验有效时解锁。
    /// offline（断网）/ expired / revoked / inactive 一律锁定。
    pub fn is_unlocked(&self) -> bool {
        self.status == "active"
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------- 设备标识 ----------------

/// 读取/首次生成设备 ID（每安装实例唯一，落盘 device.json）。
/// 激活绑定时提交服务端；设置页展示供用户核对/复制。
pub fn device_id(data_dir: &Path) -> Result<String> {
    let path = data_dir.join("device.json");
    if let Ok(raw) = std::fs::read_to_string(&path)
        && let Ok(v) = serde_json::from_str::<Value>(&raw)
        && let Some(id) = v.get("id").and_then(|i| i.as_str())
    {
        return Ok(id.to_string());
    }

    let id = uuid::Uuid::new_v4().to_string();
    std::fs::write(&path, serde_json::to_vec(&serde_json::json!({ "id": id }))?)?;
    Ok(id)
}

/// 设备名：主机名优先，缺失时用固定值（activate 协议要求 device_name 字段）
fn device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "Prism Client".into())
}

// ---------------- 本地缓存 ----------------

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedLicense {
    license: String,
    entitlement: EntitlementStatus,
}

fn cache_path(data_dir: &Path) -> PathBuf {
    data_dir.join("license_cache.json")
}

fn load_cache(data_dir: &Path) -> Option<CachedLicense> {
    let raw = std::fs::read_to_string(cache_path(data_dir)).ok()?;
    serde_json::from_str(&raw).ok()
}

fn save_cache(data_dir: &Path, cache: &CachedLicense) -> Result<()> {
    let raw = serde_json::to_vec_pretty(cache)?;
    super::store::atomic_write(&cache_path(data_dir), &raw)?;
    Ok(())
}

fn clear_cache(data_dir: &Path) {
    std::fs::remove_file(cache_path(data_dir)).ok();
}

/// 启动期从缓存恢复（不联网；联网有效性由后台周期校验任务刷新）。
/// 旧版缓存可能存有 grace（离线宽限）状态：该策略已移除，统一迁移为 offline 锁定。
pub fn cached_entitlement(data_dir: &Path) -> EntitlementStatus {
    load_cache(data_dir)
        .map(|mut c| {
            if c.entitlement.status == "grace" {
                c.entitlement.status = "offline".into();
            }
            c.entitlement
        })
        .unwrap_or_else(EntitlementStatus::inactive)
}

// ---------------- 服务端通信 ----------------

fn server_base(_settings: &UserSettings) -> String {
    DEFAULT_SERVER.trim_end_matches('/').to_string()
}

fn http_builder() -> Result<reqwest::ClientBuilder> {
    Ok(reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        // 连接阶段短超时：黑洞地址快速失败，尽快进入 IPv4 重试
        .connect_timeout(std::time::Duration::from_secs(8))
        // 不走系统代理：内核运行时系统代理指向本机，走代理会自回环；
        // 授权服务为公网 HTTPS，直连即可
        .no_proxy())
}

/// 直连失败后强制 IPv4 源重试：部分网络 IPv6 出口为黑洞（AAAA 优先 +
/// SYN 无响应），hyper 顺序尝试地址会耗尽超时。用 0.0.0.0 绑定源地址时
/// v6 目标因地址族不匹配立即跳过，连接器秒级回退到 v4 目标。
async fn send_with_v4_fallback(
    make: impl Fn(&reqwest::Client) -> reqwest::RequestBuilder,
) -> Result<reqwest::Response> {
    let mut last: Option<String> = None;
    for force_v4 in [false, true] {
        let mut b = http_builder()?;
        if force_v4 {
            b = b.local_address(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
        }
        match make(&b.build()?).send().await {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                tracing::warn!("license request (force_v4={force_v4}) failed: {e}");
                last = Some(e.to_string());
            }
        }
    }
    bail!("网络请求失败：{}（已尝试 IPv4 直连）", last.unwrap_or_default())
}

/// POST JSON 带兜底重试
async fn post_with_fallback(url: &str, body: &Value) -> Result<reqwest::Response> {
    send_with_v4_fallback(|c| {
        c.post(url)
            .header("X-Request-Source", REQUEST_SOURCE)
            .json(body)
    })
    .await
}

/// GET 带兜底重试（query: license_key）
async fn get_with_fallback(url: &str, license_key: &str) -> Result<reqwest::Response> {
    send_with_v4_fallback(|c| {
        c.get(url)
            .query(&[("license_key", license_key)])
            .header("X-Request-Source", REQUEST_SOURCE)
    })
    .await
}

// ---------------- 协议响应体 ----------------

/// POST /api/license/activate 成功响应（device_id 客户端不消费，忽略）
#[derive(Deserialize)]
struct ActivateResponse {
    ok: bool,
    #[serde(default)]
    plan: String,
    #[serde(default)]
    max_seats: Option<u32>,
}

/// GET /api/license/status 响应
#[derive(Deserialize)]
struct StatusResponse {
    valid: bool,
    #[serde(default)]
    plan: String,
    #[serde(default)]
    status: String,
    expires_at: Option<String>,
    #[serde(default)]
    max_seats: Option<u32>,
    #[serde(default)]
    used_seats: Option<u32>,
}

/// 状态查询失败分类：决定走「清除缓存」还是「离线锁定」
#[derive(Debug)]
enum StatusError {
    /// 429：携带服务端要求的重试等待秒数（透传给前端）
    RateLimited(i64),
    /// 服务端明确判定凭证无效（4xx）：应清除本地缓存
    Rejected(u16, String),
    /// 瞬态故障（网络不可达、5xx、响应解析失败）：可走离线宽限
    Transient(String),
}

/// 提取服务端错误体 message（兼容 {message} / {error: "..."} / {error:{message}}）
async fn error_message(resp: reqwest::Response) -> String {
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let message = parsed
        .get("message")
        .and_then(|m| m.as_str())
        .or_else(|| parsed.get("error").and_then(|e| e.as_str()))
        .or_else(|| {
            parsed
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
        })
        .filter(|m| !m.is_empty())
        .unwrap_or(&text);
    format!("授权服务返回错误（HTTP {status}）：{message}")
}

/// 从 429 响应体提取 retry_after（秒）；缺失/非法时回退 30
async fn retry_after_secs(resp: reqwest::Response) -> i64 {
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    body.get("retry_after")
        .and_then(|v| v.as_i64())
        .unwrap_or(30)
        .max(1)
}

/// GET /api/license/status：200 返回状态体，其余按 StatusError 分类
async fn fetch_status(base: &str, license_key: &str) -> std::result::Result<StatusResponse, StatusError> {
    let resp = get_with_fallback(&format!("{base}/api/license/status"), license_key)
        .await
        .map_err(|e| StatusError::Transient(e.to_string()))?;
    let code = resp.status().as_u16();
    if code == 429 {
        return Err(StatusError::RateLimited(retry_after_secs(resp).await));
    }
    if code >= 500 {
        return Err(StatusError::Transient(format!("HTTP {code}")));
    }
    if !resp.status().is_success() {
        return Err(StatusError::Rejected(code, error_message(resp).await));
    }
    resp.json()
        .await
        .map_err(|e| StatusError::Transient(e.to_string()))
}

// ---------------- 状态映射 ----------------

/// 将新协议响应映射为本地授权状态（沿用现有枚举语义，不新增枚举值）：
/// - plan 为 pro/team 且未过期 → active
/// - plan 为其他值（free 等）→ inactive（无 Pro 权益，保留缓存供后续状态轮询）
/// - 服务端 status=expired 或 expires_at 已过 → expired
/// - expires_at 存在 → subscription，否则 lifetime
fn map_entitlement(
    plan: &str,
    server_status: &str,
    expires_at: Option<i64>,
    max_seats: Option<u32>,
    used_seats: Option<u32>,
) -> EntitlementStatus {
    let unlocked = plan == "pro" || plan == "team";
    let expired = expires_at.map(|exp| exp <= now_secs()).unwrap_or(false) || server_status == "expired";
    let status = if !unlocked {
        "inactive"
    } else if expired {
        "expired"
    } else {
        "active"
    };
    EntitlementStatus {
        status: status.into(),
        kind: unlocked
            .then(|| if expires_at.is_some() { "subscription" } else { "lifetime" })
            .map(Into::into),
        expires_at,
        source: Some("code".into()),
        last_verified_at: now_secs(),
        plan: (!plan.is_empty()).then(|| plan.to_string()),
        max_seats,
        used_seats,
    }
}

/// 解析服务端返回的 ISO8601/RFC3339 时间戳为 Unix 秒。
/// 支持 "YYYY-MM-DDTHH:MM:SS(.fff)?(Z|±HH[:MM])?"（空格分隔日期也兼容）；失败返回 None。
fn parse_expires_at(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, rest) = s.split_once(['T', 't', ' '])?;
    let mut dp = date.split('-');
    let year: i64 = dp.next()?.parse().ok()?;
    let month: i64 = dp.next()?.parse().ok()?;
    let day: i64 = dp.next()?.parse().ok()?;

    // 拆出时间与时区偏移
    let (time, offset_secs): (&str, i64) = if let Some(z) = rest.find(['Z', 'z']) {
        (&rest[..z], 0)
    } else if let Some(p) = rest.find(['+', '-']) {
        let sign: i64 = if rest.as_bytes()[p] == b'+' { 1 } else { -1 };
        let mut op = rest[p + 1..].split(':');
        let oh: i64 = op.next().unwrap_or("0").parse().ok()?;
        let om: i64 = op.next().unwrap_or("0").parse().ok()?;
        (&rest[..p], sign * (oh * 3600 + om * 60))
    } else {
        (rest, 0)
    };

    let mut tp = time.split(':');
    let hh: i64 = tp.next()?.parse().ok()?;
    let mm: i64 = tp.next()?.parse().ok()?;
    let ss_raw = tp.next().unwrap_or("0");
    let ss: i64 = ss_raw.split('.').next().unwrap_or("0").parse().ok()?;

    Some(days_from_civil(year, month, day) * 86400 + hh * 3600 + mm * 60 + ss - offset_secs)
}

/// Howard Hinnant 的 days_from_civil 算法：公历日期 → 距 1970-01-01 的天数
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

// ---------------- 业务操作 ----------------

/// 邮箱格式校验（与服务端规则一致）
fn valid_email(e: &str) -> bool {
    let Some((local, domain)) = e.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains("..")
        && e.len() <= 254
}

/// 激活码激活（邮箱+授权码模式：首次激活绑定邮箱，之后必须一致）
pub async fn activate(
    data_dir: &Path,
    settings: &UserSettings,
    code: &str,
    email: &str,
) -> Result<EntitlementStatus> {
    let email = email.trim().to_lowercase();
    if !valid_email(&email) {
        bail!("请输入有效的邮箱地址");
    }
    let base = server_base(settings);
    let device_id_str = device_id(data_dir)?;
    let license_key = code.trim();

    let resp = post_with_fallback(
        &format!("{base}/api/license/activate"),
        &serde_json::json!({
            "license_key": license_key,
            "email": email,
            "device_id": device_id_str,
            "device_name": device_name(),
        }),
    )
    .await?;

    // 429 频率限制：把 retry_after 编进错误文本透传给前端。
    // 前端解析约定：错误字符串以 "rate_limited:" 开头，冒号后为等待秒数（整数），
    // 例如 "rate_limited:30" 表示 30 秒后可重试。
    if resp.status().as_u16() == 429 {
        let secs = retry_after_secs(resp).await;
        bail!("rate_limited:{secs}");
    }
    // 401 无效/停用、409 设备已绑定等冲突，以及其他非 2xx
    if !resp.status().is_success() {
        bail!(error_message(resp).await);
    }
    let data: ActivateResponse = resp.json().await?;
    if !data.ok {
        bail!("激活失败：服务端未确认（ok=false）");
    }

    // 激活响应不含到期时间：best-effort 拉一次状态补全 expires_at/kind；
    // 查询失败不影响激活结果（下次 verify_remote 会修正）
    let mut ent = map_entitlement(&data.plan, "", None, data.max_seats, None);
    match fetch_status(&base, license_key).await {
        Ok(s) if s.valid => {
            ent = map_entitlement(
                &s.plan,
                &s.status,
                s.expires_at.as_deref().and_then(parse_expires_at),
                s.max_seats,
                s.used_seats,
            );
        }
        Ok(_) => {}
        Err(StatusError::RateLimited(secs)) => {
            tracing::warn!("post-activate status query rate limited (retry after {secs}s)");
        }
        Err(e) => tracing::warn!("post-activate status query failed: {e:?}"),
    }

    save_cache(
        data_dir,
        &CachedLicense {
            license: license_key.to_string(),
            entitlement: ent.clone(),
        },
    )?;
    Ok(ent)
}

/// 联网校验；网络不可用时离线评估宽限期
pub async fn verify_remote(
    data_dir: &Path,
    settings: &UserSettings,
) -> Result<EntitlementStatus> {
    let Some(cache) = load_cache(data_dir) else {
        return Ok(EntitlementStatus::inactive());
    };
    let base = server_base(settings);

    match fetch_status(&base, &cache.license).await {
        Ok(s) => {
            // 服务端明确判定凭证无效（停用/删除）→ 清除缓存
            if !s.valid {
                tracing::warn!("license invalid per server (status={})", s.status);
                clear_cache(data_dir);
                return Ok(EntitlementStatus::inactive());
            }
            let ent = map_entitlement(
                &s.plan,
                &s.status,
                s.expires_at.as_deref().and_then(parse_expires_at),
                s.max_seats,
                s.used_seats,
            );
            // 吊销/过期/降级同样落盘，保证重启后状态一致
            save_cache(
                data_dir,
                &CachedLicense {
                    license: cache.license,
                    entitlement: ent.clone(),
                },
            )?;
            Ok(ent)
        }
        Err(StatusError::RateLimited(secs)) => {
            // 429 同样透传给前端（refresh_entitlement 按钮会展示）；
            // 前端解析约定同 activate："rate_limited:N"
            tracing::warn!("license status rate limited (retry after {secs}s)");
            bail!("rate_limited:{secs}");
        }
        Err(StatusError::Rejected(code, message)) => {
            tracing::warn!("license rejected by server (HTTP {code}): {message}");
            clear_cache(data_dir);
            Ok(EntitlementStatus::inactive())
        }
        Err(StatusError::Transient(e)) => {
            tracing::warn!("license status unreachable, locking Pro: {e}");
            Ok(offline_status(&cache.entitlement))
        }
    }
}

/// 断网/服务不可达时的授权状态：立即锁定 Pro（status=offline），
/// 保留方案信息（plan/kind/expires_at）供 UI 展示；联网后周期校验自动恢复。
fn offline_status(prev: &EntitlementStatus) -> EntitlementStatus {
    EntitlementStatus {
        status: "offline".into(),
        last_verified_at: prev.last_verified_at,
        ..prev.clone()
    }
}

/// 移动端商店收据上报（购买/恢复购买后调用）。
/// 新协议（askcn.de HTTP API）未提供商店收据端点，暂不支持；
/// 保留函数与签名以兼容 ipc/commands.rs 调用点，前端收到明确错误提示。
pub async fn submit_store_receipt(
    data_dir: &Path,
    settings: &UserSettings,
    store: &str,
    receipt: Value,
) -> Result<EntitlementStatus> {
    let _ = (data_dir, settings, store, receipt);
    bail!("商店内购激活暂未开放，请使用授权码 + 邮箱激活");
}

/// 移除本机授权（切换账号/退款后）
pub fn deactivate(data_dir: &Path) -> EntitlementStatus {
    clear_cache(data_dir);
    EntitlementStatus::inactive()
}
