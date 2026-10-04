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
}

fn settings_path(data_dir: &Path) -> PathBuf {
    data_dir.join("settings.json")
}

fn subscriptions_path(data_dir: &Path) -> PathBuf {
    data_dir.join("subscriptions.json")
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

/// 记录一次订阅更新（同 URL 覆盖时间戳，当前只保留最近一条）
pub fn upsert_subscription(data_dir: &Path, url: &str) -> anyhow::Result<()> {
    let mut records = load_subscriptions(data_dir);
    let now = unix_now();

    if let Some(existing) = records.iter_mut().find(|r| r.url == url) {
        existing.updated_at = now;
    } else {
        // 单订阅模型：新地址替换旧记录
        records.clear();
        records.push(SubscriptionRecord {
            url: url.to_string(),
            updated_at: now,
            name: None,
        });
    }
    write_subscriptions(data_dir, &records)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
