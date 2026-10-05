//! 本地持久化存储
//!
//! - settings.json：用户设置（端口 / 局域网 / 系统代理 / 开机自启）
//! - subscriptions.json：已导入的订阅记录（当前为单订阅模型，保留数组结构便于扩展）
//!
//! 所有文件损坏或缺失时回退默认值，绝不阻塞应用启动。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::state::UserSettings;

/// 一条订阅记录
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionRecord {
    /// 订阅地址（http/https）或本地文件路径
    pub url: String,
    /// 更新时间（Unix 秒）
    pub updated_at: i64,
    /// 可选备注名
    #[serde(default)]
    pub name: Option<String>,
    /// 是否参与配置合并（多订阅模型；旧记录缺省视为启用）
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

/// 通用原子写入：先写 .tmp 再 rename（调用方负责父目录存在）
pub fn atomic_write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let tmp = path.with_extension(
        path.extension()
            .map(|e| format!("{}.tmp", e.to_string_lossy()))
            .unwrap_or_else(|| "tmp".into()),
    );
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// 订阅内容落盘路径：data_dir/profiles/{url哈希}.yaml（由 URL 确定性派生）
pub fn profile_path(data_dir: &Path, url: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    data_dir
        .join("profiles")
        .join(format!("{:016x}.yaml", h.finish()))
}

/// proxy-provider 节点落盘路径：data_dir/providers/{subURL哈希}_{provider名哈希}.yaml
pub fn provider_path(data_dir: &Path, sub_url: &str, provider_name: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    sub_url.hash(&mut h);
    provider_name.hash(&mut h);
    data_dir
        .join("providers")
        .join(format!("{:016x}.yaml", h.finish()))
}

fn settings_path(data_dir: &Path) -> PathBuf {
    data_dir.join("settings.json")
}

fn subscriptions_path(data_dir: &Path) -> PathBuf {
    data_dir.join("subscriptions.json")
}

fn custom_rules_path(data_dir: &Path) -> PathBuf {
    data_dir.join("custom_rules.json")
}

/// 读取自定义规则（Clash 行格式，顺序即优先级）；损坏/缺失回退空列表
pub fn load_custom_rules(data_dir: &Path) -> Vec<String> {
    match std::fs::read(custom_rules_path(data_dir)) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            tracing::warn!("custom_rules.json 解析失败：{e}");
            Vec::new()
        }),
        Err(_) => Vec::new(),
    }
}

/// 原子写入自定义规则
pub fn save_custom_rules(data_dir: &Path, rules: &[String]) -> anyhow::Result<()> {
    std::fs::create_dir_all(data_dir).ok();
    let path = custom_rules_path(data_dir);
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(rules)?;
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// 读取用户设置；任何错误（不存在/损坏）都回退默认值
pub fn load_settings(data_dir: &Path) -> UserSettings {
    let path = settings_path(data_dir);
    match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<UserSettings>(&bytes) {
            Ok(settings) => settings,
            Err(e) => {
                tracing::warn!("settings.json 解析失败，使用默认设置：{e}");
                UserSettings::default()
            }
        },
        Err(_) => UserSettings::default(),
    }
}

/// 原子写入用户设置（先写临时文件再 rename，避免写一半损坏配置）
pub fn save_settings(data_dir: &Path, settings: &UserSettings) -> anyhow::Result<()> {
    std::fs::create_dir_all(data_dir).ok();
    let path = settings_path(data_dir);
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(settings)?;
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// 读取订阅记录列表
pub fn load_subscriptions(data_dir: &Path) -> Vec<SubscriptionRecord> {
    match std::fs::read(subscriptions_path(data_dir)) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            tracing::warn!("subscriptions.json 解析失败：{e}");
            Vec::new()
        }),
        Err(_) => Vec::new(),
    }
}

/// 写入完整订阅列表
fn write_subscriptions(data_dir: &Path, records: &[SubscriptionRecord]) -> anyhow::Result<()> {
    std::fs::create_dir_all(data_dir).ok();
    let path = subscriptions_path(data_dir);
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(records)?;
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// 记录一次订阅更新（同 URL 覆盖时间戳；多订阅模型，保留全部记录）
pub fn upsert_subscription(data_dir: &Path, url: &str) -> anyhow::Result<()> {
    let mut records = load_subscriptions(data_dir);
    let now = unix_now();

    if let Some(existing) = records.iter_mut().find(|r| r.url == url) {
        existing.updated_at = now;
    } else {
        records.push(SubscriptionRecord {
            url: url.to_string(),
            updated_at: now,
            name: None,
            enabled: true,
        });
    }
    write_subscriptions(data_dir, &records)
}

/// 设置订阅启用状态；返回是否存在该记录
pub fn set_subscription_enabled(
    data_dir: &Path,
    url: &str,
    enabled: bool,
) -> anyhow::Result<bool> {
    let mut records = load_subscriptions(data_dir);
    let Some(record) = records.iter_mut().find(|r| r.url == url) else {
        return Ok(false);
    };
    record.enabled = enabled;
    write_subscriptions(data_dir, &records)?;
    Ok(true)
}

/// 删除一条订阅记录（按 URL 匹配），返回是否有记录被移除
pub fn delete_subscription(data_dir: &Path, url: &str) -> anyhow::Result<bool> {
    let mut records = load_subscriptions(data_dir);
    let before = records.len();
    records.retain(|r| r.url != url);
    if records.len() == before {
        return Ok(false);
    }
    write_subscriptions(data_dir, &records)?;
    Ok(true)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
