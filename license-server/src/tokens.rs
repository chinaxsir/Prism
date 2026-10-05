//! License JWT（RS256）：私钥仅在服务端，公钥通过 GET /api/v1/public-key 给客户端 TOFU 钉取。

use anyhow::{Context, Result};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use rsa::{RsaPrivateKey, RsaPublicKey};
use rsa::pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey, LineEnding};
use serde::{Deserialize, Serialize};

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

pub struct Signer {
    encoding: EncodingKey,
    public_pem: String,
}

impl Signer {
    /// 加载私钥；缺失则生成 2048 位 RSA 并落盘
    pub fn load_or_create(path: &std::path::Path) -> Result<Signer> {
        if let Ok(pem) = std::fs::read_to_string(path)
            && let Ok(key) = RsaPrivateKey::from_pkcs8_pem(&pem)
        {
            return Signer::from_private(&key);
        }

        let mut rng = rand::thread_rng();
        let key = RsaPrivateKey::new(&mut rng, 2048).context("generate rsa key")?;
        let pem = key.to_pkcs8_pem(LineEnding::LF)?.to_string();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(path, pem).context("write private key")?;
        tracing::warn!("已生成新的授权 RSA 密钥对（旧客户端需重新激活）：{}", path.display());
        Signer::from_private(&key)
    }

    fn from_private(key: &RsaPrivateKey) -> Result<Signer> {
        let public = RsaPublicKey::from(key);
        let public_pem = public.to_public_key_pem(LineEnding::LF)?;
        Ok(Signer {
            encoding: EncodingKey::from_rsa_pem(
                key.to_pkcs8_pem(LineEnding::LF)?.as_bytes(),
            )?,
            public_pem,
        })
    }

    pub fn public_pem(&self) -> &str {
        &self.public_pem
    }

    pub fn issue(&self, claims: &LicenseClaims) -> Result<String> {
        Ok(encode(&Header::new(Algorithm::RS256), claims, &self.encoding)?)
    }
}

/// 服务端校验自己签发的 license
pub fn verify_license(license: &str, public_pem: &str) -> Result<LicenseClaims> {
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&["prism-license"]);
    let data = decode::<LicenseClaims>(
        license,
        &DecodingKey::from_rsa_pem(public_pem.as_bytes())?,
        &validation,
    )?;
    Ok(data.claims)
}
