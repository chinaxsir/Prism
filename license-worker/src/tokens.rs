//! License JWT（RS256）：纯 Rust 实现（rsa + sha2），不依赖 ring/jsonwebtoken。
//! 私钥来自 Worker Secret（LICENSE_RSA_PEM），公钥通过 GET /api/v1/public-key
//! 给客户端 TOFU 钉取。私钥缺失一律报错，绝不自动生成（无状态平台上换密钥
//! 会让全体已激活客户端的 TOFU 公钥失效）。

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rsa::RsaPrivateKey;
use rsa::pkcs1v15::Pkcs1v15Sign;
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePublicKey};
use rsa::RsaPublicKey;
use rsa::traits::SignatureScheme;
use rand_core::{CryptoRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseClaims {
    pub iss: String,
    pub iat: i64,
    pub exp: i64,
    pub jti: String,
    /// 授权来源：code | apple | google
    pub src: String,
    /// 激活码（src=code 时）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// 商店原始交易标识（src=apple/google 时）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_id: Option<String>,
    /// 绑定设备 ID
    pub dev: String,
    /// lifetime | subscription
    pub kind: String,
    /// 订阅到期时间（Unix 秒）；买断为空
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp_at: Option<i64>,
}

/// EMSA-PKCS1-v1_5 签名填充本身是确定性的，RSA 签名 API 仍要求传入 RNG，
/// 给一个零实现即可（不会影响签名结果/安全性）。
struct DummyRng;
impl RngCore for DummyRng {
    fn next_u32(&mut self) -> u32 {
        0
    }
    fn next_u64(&mut self) -> u64 {
        0
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        dest.fill(0);
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        dest.fill(0);
        Ok(())
    }
}
impl CryptoRng for DummyRng {}

pub struct Signer {
    private: RsaPrivateKey,
    public_pem: String,
}

impl Signer {
    /// 从 PKCS#8 PEM 构造；缺失/非法一律报错（不自动生成）
    pub fn from_pem(pem: &str) -> Result<Signer, String> {
        let key = RsaPrivateKey::from_pkcs8_pem(pem).map_err(|e| format!("LICENSE_RSA_PEM 解析失败: {e}"))?;
        let public = RsaPublicKey::from(&key);
        let public_pem = public
            .to_public_key_pem(rsa::pkcs8::LineEnding::LF)
            .map_err(|e| format!("公钥序列化失败: {e}"))?;
        Ok(Signer { private: key, public_pem })
    }

    pub fn public_pem(&self) -> &str {
        &self.public_pem
    }

    pub fn issue(&self, claims: &LicenseClaims) -> Result<String, String> {
        let header = serde_json::json!({ "alg": "RS256", "typ": "JWT" });
        sign_rs256(&self.private, &header, claims)
    }
}

/// RS256 签名：header/claims 任意可序列化 JSON
pub fn sign_rs256<T: Serialize>(
    key: &RsaPrivateKey,
    header: &serde_json::Value,
    claims: &T,
) -> Result<String, String> {
    let h = URL_SAFE_NO_PAD.encode(serde_json::to_vec(header).map_err(|e| e.to_string())?);
    let c = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).map_err(|e| e.to_string())?);
    let signing_input = format!("{h}.{c}");
    let digest = Sha256::digest(signing_input.as_bytes());
    let sig = Pkcs1v15Sign::new::<Sha256>()
        .sign(Some(&mut DummyRng), key, &digest)
        .map_err(|e| format!("RSA 签名失败: {e}"))?;
    Ok(format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(sig)))
}

/// 从服务账号 PEM 解析私钥（Google OAuth JWT 用）
pub fn parse_private_pem(pem: &str) -> Result<RsaPrivateKey, String> {
    RsaPrivateKey::from_pkcs8_pem(pem).map_err(|e| format!("RSA 私钥解析失败: {e}"))
}

/// 校验服务端自己签发的 license（等价原 jsonwebtoken 校验：RS256 + iss + exp）
pub fn verify_license(token: &str, public_pem: &str, now: i64) -> Result<LicenseClaims, String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err("token 段数错误".into());
    }
    let header_json = URL_SAFE_NO_PAD
        .decode(parts[0])
        .map_err(|e| format!("header base64url: {e}"))?;
    let header: serde_json::Value =
        serde_json::from_slice(&header_json).map_err(|e| format!("header json: {e}"))?;
    if header.get("alg").and_then(|v| v.as_str()) != Some("RS256") {
        return Err("alg 必须为 RS256".into());
    }

    let public = RsaPublicKey::from_public_key_pem(public_pem)
        .map_err(|e| format!("服务端公钥解析失败: {e}"))?;
    let sig = URL_SAFE_NO_PAD
        .decode(parts[2])
        .map_err(|e| format!("signature base64url: {e}"))?;
    let signing_input = format!("{}.{}", parts[0], parts[1]);
    let digest = Sha256::digest(signing_input.as_bytes());
    Pkcs1v15Sign::new::<Sha256>()
        .verify(&public, &digest, &sig)
        .map_err(|_| "签名校验失败".to_string())?;

    let payload = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|e| format!("payload base64url: {e}"))?;
    let claims: LicenseClaims =
        serde_json::from_slice(&payload).map_err(|e| format!("claims json: {e}"))?;
    if claims.iss != "prism-license" {
        return Err("iss 不匹配".into());
    }
    if claims.exp < now {
        return Err("token 已过期".into());
    }
    Ok(claims)
}

/// 缓存类型别名（供上层 Arc 化）
pub type SharedSigner = Arc<Signer>;
