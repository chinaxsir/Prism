//! Google Play Developer API 校验：服务账号 OAuth2 + 订阅/一次性购买查询。
//! 与原服务区别：HTTP 用 worker Fetch（非 reqwest）；Workers 实例不保证粘性，
//! token 不做跨请求缓存（每次收据换取一次，Google 侧配额充足）。

use serde::Deserialize;
use wasm_bindgen::JsValue;
use worker::{Fetch, Headers, Method, Request, RequestInit};

use crate::config::GoogleConfig;
use crate::tokens::{parse_private_pem, sign_rs256};

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
    #[allow(dead_code)]
    expires_in: i64,
}

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

pub struct PlayClient {
    sa: ServiceAccount,
}

impl PlayClient {
    pub fn from_config(cfg: &GoogleConfig) -> Result<Option<PlayClient>, String> {
        let Some(raw) = &cfg.service_account_json else {
            return Ok(None);
        };
        let sa: ServiceAccount =
            serde_json::from_str(raw).map_err(|e| format!("解析服务账号 JSON 失败: {e}"))?;
        Ok(Some(PlayClient { sa }))
    }

    async fn post_form(url: &str, body: String) -> Result<String, String> {
        let headers = Headers::new();
        headers
            .set("Content-Type", "application/x-www-form-urlencoded")
            .map_err(|e| e.to_string())?;
        let init = RequestInit {
            method: Method::Post,
            headers,
            body: Some(JsValue::from_str(&body)),
            ..Default::default()
        };
        let req = Request::new_with_init(url, &init).map_err(|e| e.to_string())?;
        let mut resp = Fetch::Request(req).send().await.map_err(|e| e.to_string())?;
        let status = resp.status_code();
        let text = resp.text().await.map_err(|e| e.to_string())?;
        if !(200..300).contains(&status) {
            return Err(format!("OAuth token 接口返回 {status}: {text}"));
        }
        Ok(text)
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(url: &str, token: &str) -> Result<T, String> {
        let mut req = Request::new(url, Method::Get).map_err(|e| e.to_string())?;
        req.headers_mut()
            .map_err(|e| e.to_string())?
            .set("Authorization", &format!("Bearer {token}"))
            .map_err(|e| e.to_string())?;
        let mut resp = Fetch::Request(req).send().await.map_err(|e| e.to_string())?;
        let status = resp.status_code();
        let text = resp.text().await.map_err(|e| e.to_string())?;
        if !(200..300).contains(&status) {
            return Err(format!("Play API 返回 {status}: {text}"));
        }
        serde_json::from_str(&text).map_err(|e| format!("Play API 响应解析失败: {e}: {text}"))
    }

    /// 服务账号私钥签 JWT 换 OAuth2 access token
    async fn access_token(&self) -> Result<String, String> {
        let now = (js_sys::Date::now() / 1000.0) as i64;
        let key = parse_private_pem(&self.sa.private_key)?;
        let header = serde_json::json!({
            "alg": "RS256",
            "typ": "JWT",
            "kid": self.sa.private_key_id,
        });
        let claims = serde_json::json!({
            "iss": self.sa.client_email,
            "scope": "https://www.googleapis.com/auth/androidpublisher",
            "aud": "https://oauth2.googleapis.com/token",
            "iat": now,
            "exp": now + 3600,
        });
        let assertion = sign_rs256(&key, &header, &claims)?;

        let body = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer")
            .append_pair("assertion", &assertion)
            .finish();
        let raw = Self::post_form("https://oauth2.googleapis.com/token", body).await?;
        let token: TokenResponse =
            serde_json::from_str(&raw).map_err(|e| format!("token 响应解析失败: {e}: {raw}"))?;
        Ok(token.access_token)
    }

    async fn verify_subscription(
        &self,
        package: &str,
        token: &str,
        cfg: &GoogleConfig,
    ) -> Result<NormalizedReceipt, String> {
        let access = self.access_token().await?;
        let url = format!(
            "https://androidpublisher.googleapis.com/androidpublisher/v3/applications/{package}/purchases/subscriptionsv2/tokens/{token}"
        );
        let resp: SubV2Response = Self::get_json(&url, &access).await?;

        // 1=PURCHASED、2=IN_GRACE_PERIOD 有效；3=ON_HOLD/4=PAUSED/5=EXPIRED 等无效
        if !matches!(resp.subscription_state, 1 | 2) {
            return Err(format!("订阅状态无效（state={}），未解锁", resp.subscription_state));
        }
        let item = resp
            .line_items
            .first()
            .ok_or_else(|| "订阅响应缺少 lineItems".to_string())?;
        if !cfg.subscription_products.contains(&item.product_id) {
            return Err(format!("未知订阅商品: {}", item.product_id));
        }
        let expiry = time::OffsetDateTime::parse(
            &item.expiry_time,
            &time::format_description::well_known::Rfc3339,
        )
        .map_err(|e| format!("expiryTime 解析失败: {e}"))?
        .unix_timestamp();

        Ok(NormalizedReceipt {
            original_id: token.to_string(),
            product_id: item.product_id.clone(),
            kind: "subscription".into(),
            expires_at: Some(expiry),
        })
    }

    async fn verify_product(
        &self,
        package: &str,
        product_id: &str,
        token: &str,
        cfg: &GoogleConfig,
    ) -> Result<NormalizedReceipt, String> {
        if !cfg.lifetime_products.contains(&product_id.to_string()) {
            return Err(format!("未知买断商品: {product_id}"));
        }
        let access = self.access_token().await?;
        let url = format!(
            "https://androidpublisher.googleapis.com/androidpublisher/v3/applications/{package}/purchases/products/{product_id}/tokens/{token}"
        );
        let resp: ProductResponse = Self::get_json(&url, &access).await?;

        // purchaseState: 1=PURCHASED（0 unspecified / 2 pending）
        if resp.purchase_state != 1 {
            return Err(format!("商品未处于已购买状态（state={}）", resp.purchase_state));
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
    ) -> Result<NormalizedReceipt, String> {
        if package.is_empty() {
            return Err("服务端未配置 PRISM_GOOGLE_PACKAGE".into());
        }
        if subscription {
            self.verify_subscription(package, token, cfg).await
        } else {
            self.verify_product(package, product_id, token, cfg).await
        }
    }
}
