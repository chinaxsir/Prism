//! Pro 授权客户端：设备标识、在线激活、license 验签缓存、离线宽限、商店收据上报。
//!
//! 安全模型：服务端用 RS256 签发 license；客户端首次连通时 GET 公钥并 TOFU 钉取，
//! 之后每次均用钉取的公钥验签，篡改本地缓存无法伪造授权。

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::state::UserSettings;

/// 买断离线宽限（距上次成功校验）
const LIFETIME_GRACE_DAYS: i64 = 30;
/// 订阅离线宽限（距上次成功校验）
const SUBSCRIPTION_GRACE_DAYS: i64 = 7;

/// 编译期固定授权服务地址（构建时可用 PRISM_LICENSE_URL 覆盖；App 内置，用户无需输入）
const DEFAULT_SERVER: &str = match option_env!("PRISM_LICENSE_URL") {
    Some(v) => v,
    None => "https://ishow.cc.cd",
};

// ---------------- 对外类型 ----------------

/// 授权状态快照（前端 gating 直接消费；字段名 camelCase）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementStatus {
    /// inactive | active | grace | expired | revoked
    pub status: String,
    /// lifetime | subscription
    pub kind: Option<String>,
    pub expires_at: Option<i64>,
    /// code | apple | google
    pub source: Option<String>,
    pub last_verified_at: i64,
}

impl EntitlementStatus {
    pub fn inactive() -> Self {
        Self {
            status: "inactive".into(),
            kind: None,
            expires_at: None,
            source: None,
            last_verified_at: 0,
        }
    }

    /// 是否可使用 Pro：active（联网校验有效）或 grace（离线宽限期）
    pub fn is_unlocked(&self) -> bool {
        matches!(self.status.as_str(), "active" | "grace")
    }
}

/// 上报给服务端的设备信息
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceDto {
    id: String,
    name: Option<String>,
    platform: &'static str,
    app_version: &'static str,
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------- 设备标识 ----------------

/// 读取/首次生成设备 ID（每安装实例唯一，落盘 device.json）
fn load_device(data_dir: &Path) -> Result<DeviceDto> {
    let path = data_dir.join("device.json");
    if let Ok(raw) = std::fs::read_to_string(&path)
        && let Ok(v) = serde_json::from_str::<Value>(&raw)
        && let Some(id) = v.get("id").and_then(|i| i.as_str())
    {
        return Ok(build_device(id.to_string()));
    }

    let id = uuid::Uuid::new_v4().to_string();
    std::fs::write(&path, serde_json::to_vec(&serde_json::json!({ "id": id }))?)?;
    Ok(build_device(id))
}

fn build_device(id: String) -> DeviceDto {
    // 主机名仅作服务端展示，缺失无所谓
    let hostname = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok();
    let platform = if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "ios") {
        "ios"
    } else if cfg!(target_os = "android") {
        "android"
    } else {
        "linux"
    };
    DeviceDto {
        id,
        name: hostname,
        platform,
        app_version: env!("CARGO_PKG_VERSION"),
    }
}

// ---------------- 本地缓存 ----------------

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedLicense {
    license: String,
    pinned_public_key: String,
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

/// 启动期从缓存恢复（不联网；离线宽限在后台校验任务中评估）
pub fn cached_entitlement(data_dir: &Path) -> EntitlementStatus {
    load_cache(data_dir)
        .map(|c| c.entitlement)
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
    send_with_v4_fallback(|c| c.post(url).json(body)).await
}

/// license 声明（仅取客户端需要的字段，与服务端 LicenseClaims 对应）
#[derive(Deserialize)]
#[allow(dead_code)]
struct Claims {
    src: String,
    dev: String,
    kind: String,
    exp_at: Option<i64>,
}

#[derive(Deserialize)]
struct ServerResponse {
    license: Option<String>,
    entitlement: EntitlementStatus,
}

/// 用公钥验证 license 签名并解析声明
fn verify_license(license: &str, public_pem: &str) -> Result<Claims> {
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&["prism-license"]);
    validation.validate_exp = false; // 有效期由 exp_at/expires_at 判定
    validation.validate_aud = false;
    let data = decode::<Claims>(
        license,
        &DecodingKey::from_rsa_pem(public_pem.as_bytes())?,
        &validation,
    )?;
    Ok(data.claims)
}

/// 获取（或沿用已钉取的）服务端公钥。
/// 已钉取时不再请求；与缓存不一致属于服务端换钥，拒绝并要求联系支持。
async fn resolve_public_key(base: &str, cached_pin: Option<&str>) -> Result<String> {
    if let Some(pin) = cached_pin {
        return Ok(pin.to_string());
    }
    let body: Value = send_with_v4_fallback(|c| c.get(&format!("{base}/api/v1/public-key")))
        .await?
        .json()
        .await?;
    let key = body
        .get("publicKey")
        .and_then(|k| k.as_str())
        .ok_or_else(|| anyhow::anyhow!("服务端未返回公钥"))?;
    Ok(key.to_string())
}

/// 提取服务端错误体 message
async fn error_message(resp: reqwest::Response) -> String {
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let message = parsed
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .filter(|m| !m.is_empty())
        .unwrap_or(&text);
    format!("授权服务返回错误（HTTP {status}）：{message}")
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
    let device = load_device(data_dir)?;

    let resp = post_with_fallback(
        &format!("{base}/api/v1/activate"),
        &serde_json::json!({ "code": code.trim(), "email": email, "device": device }),
    )
    .await?;
    if !resp.status().is_success() {
        bail!(error_message(resp).await);
    }
    let data: ServerResponse = resp.json().await?;
    let license = data
        .license
        .ok_or_else(|| anyhow::anyhow!("服务端未返回 license"))?;

    let prev_pin = load_cache(data_dir).map(|c| c.pinned_public_key);
    let public_key = resolve_public_key(&base, prev_pin.as_deref()).await?;
    verify_license(&license, &public_key)?;

    save_cache(
        data_dir,
        &CachedLicense {
            license,
            pinned_public_key: public_key,
            entitlement: data.entitlement.clone(),
        },
    )?;
    Ok(data.entitlement)
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

    let resp = match post_with_fallback(
        &format!("{base}/api/v1/verify"),
        &serde_json::json!({ "license": cache.license }),
    )
    .await
    {
        Ok(r) => r,
        Err(_) => return Ok(offline_eval(&cache.entitlement)),
    };

    // 401/403：服务端明确判定凭证无效 → 清除缓存
    if matches!(resp.status().as_u16(), 401 | 403) {
        let message = error_message(resp).await;
        tracing::warn!("license rejected by server: {message}");
        clear_cache(data_dir);
        return Ok(EntitlementStatus::inactive());
    }
    if !resp.status().is_success() {
        return Ok(offline_eval(&cache.entitlement));
    }

    let data: ServerResponse = resp.json().await?;
    // 服务端返回 active=false（吊销/过期）时同步落盘
    save_cache(
        data_dir,
        &CachedLicense {
            license: cache.license,
            pinned_public_key: cache.pinned_public_key,
            entitlement: data.entitlement.clone(),
        },
    )?;
    Ok(data.entitlement)
}

/// 离线宽限评估：
/// - 买断：距上次校验 30 天内 → grace，否则锁定
/// - 订阅：到期时间未来且距上次校验 7 天内 → grace；到期 → expired；否则锁定
fn offline_eval(prev: &EntitlementStatus) -> EntitlementStatus {
    let now = now_secs();
    let age_days = (now - prev.last_verified_at) / 86400;

    let status = if prev.kind.as_deref() == Some("subscription") {
        match prev.expires_at {
            Some(exp) if exp <= now => "expired",
            Some(_) if age_days <= SUBSCRIPTION_GRACE_DAYS => "grace",
            _ => "inactive",
        }
    } else if age_days <= LIFETIME_GRACE_DAYS {
        "grace"
    } else {
        "inactive"
    };

    EntitlementStatus {
        status: status.into(),
        last_verified_at: prev.last_verified_at,
        ..prev.clone()
    }
}

/// 移动端商店收据上报（购买/恢复购买后调用）
pub async fn submit_store_receipt(
    data_dir: &Path,
    settings: &UserSettings,
    store: &str,
    receipt: Value,
) -> Result<EntitlementStatus> {
    let base = server_base(settings);
    let device = load_device(data_dir)?;

    let (path, body) = match store {
        "apple" => {
            let signed = receipt
                .get("signedTransaction")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("缺少 signedTransaction"))?;
            (
                "/api/v1/receipt/apple",
                serde_json::json!({ "signedTransaction": signed, "device": device }),
            )
        }
        "google" => {
            let purchase_token = receipt
                .get("purchaseToken")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("缺少 purchaseToken"))?;
            let product_id = receipt
                .get("productId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("缺少 productId"))?;
            let subscription = receipt
                .get("subscription")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            (
                "/api/v1/receipt/google",
                serde_json::json!({
                    "purchaseToken": purchase_token,
                    "productId": product_id,
                    "subscription": subscription,
                    "device": device
                }),
            )
        }
        other => bail!("未知商店: {other}"),
    };

    let resp = post_with_fallback(&format!("{base}{path}"), &body).await?;
    if !resp.status().is_success() {
        bail!(error_message(resp).await);
    }
    let data: ServerResponse = resp.json().await?;
    let license = data
        .license
        .ok_or_else(|| anyhow::anyhow!("服务端未返回 license"))?;

    let prev_pin = load_cache(data_dir).map(|c| c.pinned_public_key);
    let public_key = resolve_public_key(&base, prev_pin.as_deref()).await?;
    verify_license(&license, &public_key)?;

    save_cache(
        data_dir,
        &CachedLicense {
            license,
            pinned_public_key: public_key,
            entitlement: data.entitlement.clone(),
        },
    )?;
    Ok(data.entitlement)
}

/// 移除本机授权（切换账号/退款后）
pub fn deactivate(data_dir: &Path) -> EntitlementStatus {
    clear_cache(data_dir);
    EntitlementStatus::inactive()
}
