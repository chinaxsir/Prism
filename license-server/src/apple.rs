//! StoreKit2 收据校验：JWS x5c 证书链验证到 Apple Root CA - G3，再用叶子公钥验 JWS 签名。

use anyhow::{bail, Result};
use base64::{Engine, engine::general_purpose::STANDARD, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::Deserialize;
use x509_parser::prelude::*;

use crate::config::AppleConfig;

const APPLE_ROOT_G3: &[u8] = include_bytes!("../assets/AppleRootCA-G3.cer");

/// 校验通过后的标准化收据
pub struct NormalizedReceipt {
    pub original_id: String,
    pub product_id: String,
    /// lifetime | subscription
    pub kind: String,
    pub expires_at: Option<i64>,
}

#[derive(Deserialize)]
struct JwsHeader {
    alg: String,
    x5c: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoreKitPayload {
    bundle_id: String,
    product_id: String,
    original_transaction_id: String,
    expiration_date: Option<i64>,
    revocation_date: Option<i64>,
    #[serde(rename = "type")]
    tx_type: String,
}

/// DER 证书 → PEM 文本
fn der_to_pem(der: &[u8]) -> String {
    let b64 = STANDARD.encode(der);
    let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(chunk).unwrap());
        pem.push('\n');
    }
    pem.push_str("-----END CERTIFICATE-----\n");
    pem
}

pub fn verify(jws: &str, cfg: &AppleConfig) -> Result<NormalizedReceipt> {
    let parts: Vec<&str> = jws.split('.').collect();
    if parts.len() != 3 {
        bail!("收据不是合法 JWS 格式");
    }

    // ---- 1. 解析 header.x5c 证书链 ----
    let header_json = URL_SAFE_NO_PAD.decode(parts[0])?;
    let header: JwsHeader = serde_json::from_slice(&header_json)?;
    if header.x5c.is_empty() {
        bail!("收据缺少 x5c 证书链");
    }
    // 证书 DER 全部转 owned 存放；解析在需要时临时进行，避免跨语句借用
    let mut certs_der: Vec<Vec<u8>> = Vec::new();
    for cert_b64 in &header.x5c {
        certs_der.push(STANDARD.decode(cert_b64)?);
    }
    let root_der = APPLE_ROOT_G3.to_vec();

    // ---- 2. 逐级验签，最终锚定内置 Apple Root G3 ----
    for i in 0..certs_der.len() {
        let (_, cert) = X509Certificate::from_der(&certs_der[i])?;
        let issuer_owned = certs_der.get(i + 1).cloned().unwrap_or_else(|| root_der.clone());
        let (_, issuer) = X509Certificate::from_der(&issuer_owned)?;
        cert.verify_signature(Some(&issuer.public_key()))
            .map_err(|e| anyhow::anyhow!("证书链验签失败: {e}"))?;
    }

    // ---- 3. 用叶子证书公钥验证 JWS 签名（jsonwebtoken 标准校验）----
    let leaf_pem = der_to_pem(&certs_der[0]);
    let alg = match header.alg.as_str() {
        "ES256" => Algorithm::ES256,
        "RS256" => Algorithm::RS256,
        other => bail!("不支持的 JWS 算法: {other}"),
    };
    let decoding = if alg == Algorithm::ES256 {
        DecodingKey::from_ec_pem(leaf_pem.as_bytes())?
    } else {
        DecodingKey::from_rsa_pem(leaf_pem.as_bytes())?
    };
    let mut validation = Validation::new(alg);
    validation.validate_exp = false; // 历史交易可能已过期，订阅有效期由 expirationDate 判定
    validation.required_spec_claims.clear();
    validation.validate_aud = false;
    let payload: StoreKitPayload = decode(jws, &decoding, &validation)?.claims;

    // ---- 4. 业务字段校验 ----
    if payload.bundle_id != cfg.bundle_id {
        bail!("收据 bundleId 不匹配: {}", payload.bundle_id);
    }
    if payload.revocation_date.is_some() {
        bail!("交易已被 Apple 吊销");
    }

    let (kind, expires_at) = if cfg.subscription_products.contains(&payload.product_id) {
        let expires = payload
            .expiration_date
            .ok_or_else(|| anyhow::anyhow!("订阅收据缺少 expirationDate"))?;
        ("subscription", Some(expires / 1000))
    } else if cfg.lifetime_products.contains(&payload.product_id) {
        ("lifetime", None)
    } else {
        bail!("未知商品 ID: {}", payload.product_id);
    };

    if kind == "subscription" && payload.tx_type != "Auto-Renewable Subscription" {
        tracing::warn!("商品标记为订阅但交易类型为 {}", payload.tx_type);
    }

    Ok(NormalizedReceipt {
        original_id: payload.original_transaction_id,
        product_id: payload.product_id,
        kind: kind.to_string(),
        expires_at,
    })
}
