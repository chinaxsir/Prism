//! Google Play Developer API 校验：服务账号 OAuth2 + 查询订阅/一次性购买状态。

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use jsonwebtoken::{Algorithm, EncodingKey, Header, Validation, encode};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::config::GoogleConfig;

/// 校验通过后的标准化收据（字段同 apple::NormalizedReceipt）
pub struct NormalizedReceipt {
    pub original_id: String,
    pub product_id: String,
    pub kind: String,
    pub expires_at: Option<i64>,
}

#[derive(Deserialize)]
struct ServiceAccount {
    client_email: String,
    private_key: String,
    private_key_id: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: i64,
}

// --- subscriptionsv2 响应（仅取需要的字段；未知字段忽略）---

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubV2Response {
    subscription_state: i64,
    line_items: Vec<SubLineItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubLineItem {
    product_id: String,
    expiry_time: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProductResponse {
    purchase_state: i64,
    purchase_time_millis: String,
}

/// Play 授权客户端（token 缓存）
pub struct PlayClient {
    sa: ServiceAccount,
    http: reqwest::Client,
    token: Arc<Mutex<Option<(String, Instant)>>>,
}

impl PlayClient {
    pub fn from_config(cfg: &GoogleConfig) -> Result<Option<PlayClient>> {
        let Some(path) = &cfg.service_account_json else {
            return Ok(None);
        };
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("read service account: {}", path.display()))?;
        let sa: ServiceAccount = serde_json::from_str(&raw).context("parse service account json")?;
        Ok(Some(PlayClient {
            sa,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()?,
            token: Arc::new(Mutex::new(None)),
        }))
    }

    /// 用服务账号私钥签 JWT 换取 OAuth2 access token（带缓存）
    async fn access_token(&self) -> Result<String> {
        {
            let guard = self.token.lock().await;
            if let Some((token, expiry)) = guard.as_ref()
                && expiry.saturating_duration_since(Instant::now()).as_secs() > 60
            {
                return Ok(token.clone());
            }
        }

        let iat = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(self.sa.private_key_id.clone());
        let claims = serde_json::json!({
            "iss": self.sa.client_email,
            "scope": "https://www.googleapis.com/auth/androidpublisher",
            "aud": "https://oauth2.googleapis.com/token",
            "iat": iat,
            "exp": iat + 3600,
        });
        let assertion = encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_pem(self.sa.private_key.as_bytes())?,
        )?;

        let resp: TokenResponse = self
            .http
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &assertion),
            ])
            .send()
            .await?
            .json()
            .await?;

        let expiry = Instant::now() + Duration::from_secs(resp.expires_in.max(60) as u64);
        *self.token.lock().await = Some((resp.access_token.clone(), expiry));
        Ok(resp.access_token)
    }

    /// 校验订阅（subscriptionsv2）
    async fn verify_subscription(
        &self,
        package: &str,
        token: &str,
        cfg: &GoogleConfig,
    ) -> Result<NormalizedReceipt> {
        if !cfg.subscription_products.is_empty() { /* 映射在下面用 product_id 做 */ }
        let access = self.access_token().await?;
        let url = format!(
            "https://androidpublisher.googleapis.com/androidpublisher/v3/applications/{package}/purchases/subscriptionsv2/tokens/{token}"
        );
        let resp: SubV2Response = self
            .http
            .get(url)
            .bearer_auth(access)
            .send()
            .await?
            .json()
            .await?;

        // 1=PURCHASED、2=IN_GRACE_PERIOD 为有效；3=ON_HOLD/4=PAUSED/5=EXPIRED 等无效
        if !matches!(resp.subscription_state, 1 | 2) {
            bail!("订阅状态无效（state={}），未解锁", resp.subscription_state);
        }
        let item = resp
            .line_items
            .first()
            .context("订阅响应缺少 lineItems")?;
        if !cfg.subscription_products.contains(&item.product_id) {
            bail!("未知订阅商品: {}", item.product_id);
        }
        let expiry = time::OffsetDateTime::parse(
            &item.expiry_time,
            &time::format_description::well_known::Rfc3339,
        )?
        .unix_timestamp();

        Ok(NormalizedReceipt {
            original_id: token.to_string(),
            product_id: item.product_id.clone(),
            kind: "subscription".into(),
            expires_at: Some(expiry),
        })
    }

    /// 校验一次性购买（purchases/products）→ 视为买断
    async fn verify_product(
        &self,
        package: &str,
        product_id: &str,
        token: &str,
        cfg: &GoogleConfig,
    ) -> Result<NormalizedReceipt> {
        if !cfg.lifetime_products.contains(&product_id.to_string()) {
            bail!("未知买断商品: {product_id}");
        }
        let access = self.access_token().await?;
        let url = format!(
            "https://androidpublisher.googleapis.com/androidpublisher/v3/applications/{package}/purchases/products/{product_id}/tokens/{token}"
        );
        let resp: ProductResponse = self
            .http
            .get(url)
            .bearer_auth(access)
            .send()
            .await?
            .json()
            .await?;

        // purchaseState: 1=PURCHASED（0 unspecified / 2 pending）
        if resp.purchase_state != 1 {
            bail!("商品未处于已购买状态（state={}）", resp.purchase_state);
        }

        Ok(NormalizedReceipt {
            original_id: format!("{}#{}", product_id, resp.purchase_time_millis),
            product_id: product_id.to_string(),
            kind: "lifetime".into(),
            expires_at: None,
        })
    }

    pub async fn verify(
        &self,
        package: &str,
        product_id: &str,
        token: &str,
        subscription: bool,
        cfg: &GoogleConfig,
    ) -> Result<NormalizedReceipt> {
        if package.is_empty() {
            bail!("服务端未配置 PRISM_GOOGLE_PACKAGE");
        }
        if subscription {
            self.verify_subscription(package, token, cfg).await
        } else {
            self.verify_product(package, product_id, token, cfg).await
        }
    }
}

/// 供启动期探测配置错误：构造一次（不发起网络请求）
pub fn build_client(cfg: &GoogleConfig) -> Result<Option<PlayClient>> {
    PlayClient::from_config(cfg)
}

/// 抑制未使用 import（Validation 在部分特性组合下不用）
const _: fn() = || {
    let _ = Validation::new(Algorithm::RS256);
};
