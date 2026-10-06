//! StoreKit2 收据校验：JWS x5c 证书链验证到内置 Apple Root CA - G3，
//! 再用叶子证书公钥验 JWS 签名。
//!
//! wasm32 适配：不用 jsonwebtoken / x509-parser 的 verify 特性（依赖 ring）。
//! 证书解析用 x509-parser 纯解析（default-features=false），签名验证用
//! RustCrypto（p256/p384/rsa + sha2，纯 Rust）。tbsCertificate 的原始 DER
//! 在 0.17 里是 pub(crate)，因此自行按 DER TLV 切出（签名即覆盖该 TLV）。

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use p256::ecdsa::signature::Verifier as _;
use rsa::pkcs1v15::Pkcs1v15Sign;
use rsa::pkcs8::DecodePublicKey as _;
use rsa::traits::SignatureScheme as _;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use x509_parser::asn1_rs::Oid;
use x509_parser::prelude::*;

use crate::config::AppleConfig;

const APPLE_ROOT_G3: &[u8] = include_bytes!("../assets/AppleRootCA-G3.cer");

// 关键 OID（弧线表示）
const OID_EC_P256: &[u64] = &[1, 2, 840, 10045, 3, 1, 7];
const OID_EC_P384: &[u64] = &[1, 3, 132, 0, 34];
// ecdsa-with-SHA256 / SHA384
const OID_ECDSA_SHA256: &[u64] = &[1, 2, 840, 10045, 4, 3, 2];
const OID_ECDSA_SHA384: &[u64] = &[1, 2, 840, 10045, 4, 3, 3];
// sha256WithRSAEncryption
const OID_RSA_SHA256: &[u64] = &[1, 2, 840, 113549, 1, 1, 11];

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
    _tx_type: String,
}

/// 读 DER 短/长形式长度，返回 (值, 长度字段占用字节数)
fn read_der_len(buf: &[u8]) -> Result<(usize, usize), String> {
    let first = *buf.first().ok_or_else(|| "DER 长度缺失".to_string())?;
    if first & 0x80 == 0 {
        Ok((first as usize, 1))
    } else {
        let n = (first & 0x7f) as usize;
        if n == 0 || n > 4 {
            return Err("DER 长度非法".into());
        }
        let mut v = 0usize;
        for &b in buf.get(1..1 + n).ok_or_else(|| "DER 长度截断".to_string())? {
            v = (v << 8) | b as usize;
        }
        Ok((v, 1 + n))
    }
}

/// 切出证书 DER 中 tbsCertificate 的完整 TLV（验签覆盖的字节）
fn tbs_certificate_tlv(cert_der: &[u8]) -> Result<&[u8], String> {
    if cert_der.first() != Some(&0x30) {
        return Err("证书不是 DER SEQUENCE".into());
    }
    let (outer_len, outer_len_hdr) = read_der_len(&cert_der[1..])?;
    let outer_hdr = 1 + outer_len_hdr;
    let content = cert_der
        .get(outer_hdr..outer_hdr + outer_len)
        .ok_or_else(|| "证书外层 SEQUENCE 截断".to_string())?;
    if content.first() != Some(&0x30) {
        return Err("tbsCertificate 不是 SEQUENCE".into());
    }
    let (inner_len, inner_len_hdr) = read_der_len(&content[1..])?;
    let tlv_len = 1 + inner_len_hdr + inner_len;
    content
        .get(..tlv_len)
        .ok_or_else(|| "tbsCertificate TLV 截断".to_string())
}

fn parse_cert(der: &[u8]) -> Result<X509Certificate<'_>, String> {
    parse_x509_certificate(der)
        .map(|(_, c)| c)
        .map_err(|e| format!("证书解析失败: {e}"))
}

/// 判断 SPKI 是否为指定弧线的 EC 曲线（secp256r1 / secp384r1）
fn ec_curve_is(spki: &SubjectPublicKeyInfo, arcs: &[u64]) -> bool {
    let expected = Oid::from(arcs).expect("内置曲线 OID 合法");
    spki
        .algorithm
        .parameters
        .as_ref()
        .and_then(|any| Oid::from_der(any.as_bytes()).ok().map(|(_, o)| o))
        .map(|oid| oid == expected)
        .unwrap_or(false)
}

/// 验证 cert_der 的签名由 issuer_der 证书公钥签发
fn verify_cert_signature(cert_der: &[u8], issuer_der: &[u8]) -> Result<(), String> {
    let cert = parse_cert(cert_der)?;
    let issuer = parse_cert(issuer_der)?;

    let msg = tbs_certificate_tlv(cert_der)?;
    let sig = &cert.signature_value.data;
    let alg = &cert.signature_algorithm.algorithm;
    let spki = issuer.public_key();
    let spki_raw = spki.raw;

    if alg == &Oid::from(OID_ECDSA_SHA256).unwrap() {
        if !ec_curve_is(spki, OID_EC_P256) {
            return Err("SHA256 证书签名要求 P-256 签发者".into());
        }
        let vk = p256::ecdsa::VerifyingKey::from_public_key_der(spki_raw)
            .map_err(|e| format!("P-256 公钥解析失败: {e}"))?;
        let fixed = p256::ecdsa::Signature::from_der(sig)
            .map_err(|e| format!("ECDSA DER 签名解析失败: {e}"))?;
        vk.verify(msg, &fixed)
            .map_err(|_| "证书链 ECDSA-P256/SHA256 验签失败".to_string())
    } else if alg == &Oid::from(OID_ECDSA_SHA384).unwrap() {
        if !ec_curve_is(spki, OID_EC_P384) {
            return Err("SHA384 证书签名要求 P-384 签发者".into());
        }
        let vk = p384::ecdsa::VerifyingKey::from_public_key_der(spki_raw)
            .map_err(|e| format!("P-384 公钥解析失败: {e}"))?;
        let fixed = p384::ecdsa::Signature::from_der(sig)
            .map_err(|e| format!("ECDSA DER 签名解析失败: {e}"))?;
        vk.verify(msg, &fixed)
            .map_err(|_| "证书链 ECDSA-P384/SHA384 验签失败".to_string())
    } else if alg == &Oid::from(OID_RSA_SHA256).unwrap() {
        let pubkey = rsa::RsaPublicKey::from_public_key_der(spki_raw)
            .map_err(|e| format!("RSA 公钥解析失败: {e}"))?;
        Pkcs1v15Sign::new::<Sha256>()
            .verify(&pubkey, &Sha256::digest(msg), sig)
            .map_err(|_| "证书链 RSA/SHA256 验签失败".to_string())
    } else {
        Err(format!("不支持的证书签名算法 OID: {alg}"))
    }
}

/// 用叶子证书公钥验 JWS：ES256（裸 r||s）/ RS256（PKCS1v15）
fn verify_jws_with_leaf(jws: &str, leaf_der: &[u8], alg: &str) -> Result<(), String> {
    let parts: Vec<&str> = jws.split('.').collect();
    let signing_input = format!("{}.{}", parts[0], parts[1]);
    let sig = URL_SAFE_NO_PAD
        .decode(parts[2])
        .map_err(|e| format!("JWS 签名 base64url: {e}"))?;

    let leaf = parse_cert(leaf_der)?;
    let spki_raw = leaf.public_key().raw;

    match alg {
        "ES256" => {
            if !ec_curve_is(leaf.public_key(), OID_EC_P256) {
                return Err("ES256 要求 P-256 叶子证书".into());
            }
            let vk = p256::ecdsa::VerifyingKey::from_public_key_der(spki_raw)
                .map_err(|e| format!("叶子 P-256 公钥解析失败: {e}"))?;
            let fixed = p256::ecdsa::Signature::from_slice(&sig)
                .map_err(|e| format!("ES256 签名长度/格式错误: {e}"))?;
            vk.verify(signing_input.as_bytes(), &fixed)
                .map_err(|_| "JWS ES256 验签失败".to_string())
        }
        "RS256" => {
            let pubkey = rsa::RsaPublicKey::from_public_key_der(spki_raw)
                .map_err(|e| format!("叶子 RSA 公钥解析失败: {e}"))?;
            Pkcs1v15Sign::new::<Sha256>()
                .verify(&pubkey, &Sha256::digest(signing_input.as_bytes()), &sig)
                .map_err(|_| "JWS RS256 验签失败".to_string())
        }
        other => Err(format!("不支持的 JWS 算法: {other}")),
    }
}

pub fn verify(jws: &str, cfg: &AppleConfig) -> Result<NormalizedReceipt, String> {
    let parts: Vec<&str> = jws.split('.').collect();
    if parts.len() != 3 {
        return Err("收据不是合法 JWS 格式".into());
    }

    // ---- 1. 解析 header.x5c ----
    let header_json = URL_SAFE_NO_PAD
        .decode(parts[0])
        .map_err(|e| format!("header base64url: {e}"))?;
    let header: JwsHeader =
        serde_json::from_slice(&header_json).map_err(|e| format!("header json: {e}"))?;
    if header.x5c.is_empty() {
        return Err("收据缺少 x5c 证书链".into());
    }
    if header.alg != "ES256" && header.alg != "RS256" {
        return Err(format!("不支持的 JWS 算法: {}", header.alg));
    }
    let mut certs_der: Vec<Vec<u8>> = Vec::new();
    for cert_b64 in &header.x5c {
        certs_der.push(STANDARD.decode(cert_b64).map_err(|e| format!("x5c base64: {e}"))?);
    }
    let root_der = APPLE_ROOT_G3.to_vec();

    // ---- 2. 逐级验签锚定内置 Root G3 ----
    for i in 0..certs_der.len() {
        let issuer_der = certs_der.get(i + 1).cloned().unwrap_or_else(|| root_der.clone());
        verify_cert_signature(&certs_der[i], &issuer_der)?;
    }

    // ---- 3. 叶子公钥验 JWS ----
    verify_jws_with_leaf(jws, &certs_der[0], &header.alg)?;

    // ---- 4. 业务字段 ----
    let payload_raw = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|e| format!("payload base64url: {e}"))?;
    let payload: StoreKitPayload =
        serde_json::from_slice(&payload_raw).map_err(|e| format!("payload json: {e}"))?;

    if payload.bundle_id != cfg.bundle_id {
        return Err(format!("收据 bundleId 不匹配: {}", payload.bundle_id));
    }
    if payload.revocation_date.is_some() {
        return Err("交易已被 Apple 吊销".into());
    }

    let (kind, expires_at) = if cfg.subscription_products.contains(&payload.product_id) {
        let expires = payload
            .expiration_date
            .ok_or_else(|| "订阅收据缺少 expirationDate".to_string())?;
        ("subscription", Some(expires / 1000))
    } else if cfg.lifetime_products.contains(&payload.product_id) {
        ("lifetime", None)
    } else {
        return Err(format!("未知商品 ID: {}", payload.product_id));
    };

    Ok(NormalizedReceipt {
        original_id: payload.original_transaction_id,
        product_id: payload.product_id,
        kind: kind.to_string(),
        expires_at,
    })
}
