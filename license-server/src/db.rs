//! SQLite 数据访问层。单进程部署，用 Mutex 包裹单连接即可。

use std::sync::Arc;

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::Serialize;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

/// 激活码记录（部分字段仅持久化与管理接口使用，可能不被业务逻辑读取）
#[allow(dead_code)]
pub struct CodeRecord {
    pub code: String,
    pub kind: String,
    pub duration_days: Option<i64>,
    pub max_devices: i64,
    pub status: String,
    pub anchor: Option<i64>,
    pub note: Option<String>,
    pub created_at: i64,
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS codes(
  code TEXT PRIMARY KEY,
  kind TEXT NOT NULL,                  -- lifetime | subscription
  duration_days INTEGER,               -- subscription: 有效天数
  max_devices INTEGER NOT NULL DEFAULT 3,
  status TEXT NOT NULL DEFAULT 'active',
  note TEXT,
  created_at INTEGER NOT NULL,
  anchor INTEGER                       -- subscription: 首次激活时间锚点
);
CREATE TABLE IF NOT EXISTS code_devices(
  code TEXT NOT NULL,
  device_id TEXT NOT NULL,
  device_name TEXT,
  platform TEXT,
  app_version TEXT,
  activated_at INTEGER NOT NULL,
  last_seen INTEGER NOT NULL,
  PRIMARY KEY(code, device_id)
);
CREATE TABLE IF NOT EXISTS store_purchases(
  store TEXT NOT NULL,                 -- apple | google
  original_id TEXT NOT NULL,           -- apple originalTransactionId / google purchaseToken
  product_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  expires_at INTEGER,                  -- subscription: Unix 秒
  raw TEXT,
  first_seen INTEGER NOT NULL,
  last_seen INTEGER NOT NULL,
  PRIMARY KEY(store, original_id)
);
CREATE TABLE IF NOT EXISTS store_devices(
  store TEXT NOT NULL,
  original_id TEXT NOT NULL,
  device_id TEXT NOT NULL,
  device_name TEXT,
  platform TEXT,
  app_version TEXT,
  last_seen INTEGER NOT NULL,
  PRIMARY KEY(store, original_id, device_id)
);
"#;

impl Db {
    pub async fn open(path: &std::path::Path) -> Result<Db> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)
            .with_context(|| format!("open db failed: {}", path.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;",
        )?;
        conn.execute_batch(SCHEMA)?;
        Ok(Db { conn: Arc::new(Mutex::new(conn)) })
    }

    pub async fn get_code(&self, code: &str) -> Result<Option<CodeRecord>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT code,kind,duration_days,max_devices,status,anchor,note,created_at
             FROM codes WHERE code = ?1",
        )?;
        let mut rows = stmt.query([code])?;
        match rows.next()? {
            Some(row) => Ok(Some(CodeRecord {
                code: row.get(0)?,
                kind: row.get(1)?,
                duration_days: row.get(2)?,
                max_devices: row.get(3)?,
                status: row.get(4)?,
                anchor: row.get(5)?,
                note: row.get(6)?,
                created_at: row.get(7)?,
            })),
            None => Ok(None),
        }
    }

    /// 设置订阅锚点（首次激活时间）
    pub async fn set_anchor(&self, code: &str, anchor: i64) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute("UPDATE codes SET anchor = ?1 WHERE code = ?2 AND anchor IS NULL",
            rusqlite::params![anchor, code])?;
        Ok(())
    }

    pub async fn device_count(&self, code: &str) -> Result<i64> {
        let conn = self.conn.lock().await;
        conn.query_row("SELECT COUNT(*) FROM code_devices WHERE code = ?1", [code], |r| r.get(0))
            .map_err(Into::into)
    }

    /// 探测指定设备是否已绑定激活码
    pub async fn is_device_bound(&self, code: &str, device_id: &str) -> Result<bool> {
        let conn = self.conn.lock().await;
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM code_devices WHERE code=?1 AND device_id=?2",
            rusqlite::params![code, device_id],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    pub async fn upsert_code_device(
        &self,
        code: &str,
        d: &super::handlers::DeviceInfo,
    ) -> Result<bool> {
        let conn = self.conn.lock().await;
        let exists: bool = conn.query_row(
            "SELECT 1 FROM code_devices WHERE code=?1 AND device_id=?2",
            rusqlite::params![code, d.id],
            |_| Ok(true),
        ).unwrap_or(false);
        let now = now_secs();
        if exists {
            conn.execute(
                "UPDATE code_devices SET last_seen=?1, device_name=?2, platform=?3, app_version=?4
                 WHERE code=?5 AND device_id=?6",
                rusqlite::params![now, d.name, d.platform, d.app_version, code, d.id],
            )?;
            Ok(false)
        } else {
            conn.execute(
                "INSERT INTO code_devices
                   (code,device_id,device_name,platform,app_version,activated_at,last_seen)
                 VALUES (?1,?2,?3,?4,?5,?6,?6)",
                rusqlite::params![code, d.id, d.name, d.platform, d.app_version, now],
            )?;
            Ok(true)
        }
    }

    /// 移除单条激活设备绑定
    pub async fn unbind_device(&self, code: &str, device_id: &str) -> Result<bool> {
        let conn = self.conn.lock().await;
        let n = conn.execute(
            "DELETE FROM code_devices WHERE code=?1 AND device_id=?2",
            rusqlite::params![code, device_id],
        )?;
        Ok(n > 0)
    }

    pub async fn upsert_store_purchase(
        &self,
        store: &str,
        original_id: &str,
        product_id: &str,
        kind: &str,
        expires_at: Option<i64>,
        raw: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().await;
        let now = now_secs();
        conn.execute(
            "INSERT INTO store_purchases(store,original_id,product_id,kind,expires_at,raw,first_seen,last_seen)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?7)
             ON CONFLICT(store,original_id) DO UPDATE SET
               product_id=excluded.product_id, kind=excluded.kind,
               expires_at=excluded.expires_at, raw=excluded.raw, last_seen=?7",
            rusqlite::params![store, original_id, product_id, kind, expires_at, raw, now],
        )?;
        Ok(())
    }

    pub async fn get_store_purchase(
        &self,
        store: &str,
        original_id: &str,
    ) -> Result<Option<(String, String, Option<i64>)>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT product_id,kind,expires_at FROM store_purchases
             WHERE store=?1 AND original_id=?2",
        )?;
        let mut rows = stmt.query(rusqlite::params![store, original_id])?;
        match rows.next()? {
            Some(r) => Ok(Some((r.get(0)?, r.get(1)?, r.get(2)?))),
            None => Ok(None),
        }
    }

    pub async fn upsert_store_device(
        &self,
        store: &str,
        original_id: &str,
        d: &super::handlers::DeviceInfo,
    ) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO store_devices(store,original_id,device_id,device_name,platform,app_version,last_seen)
             VALUES (?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(store,original_id,device_id) DO UPDATE SET
               device_name=excluded.device_name, platform=excluded.platform,
               app_version=excluded.app_version, last_seen=excluded.last_seen",
            rusqlite::params![store, original_id, d.id, d.name, d.platform, d.app_version, now_secs()],
        )?;
        Ok(())
    }

    // ---- 管理接口 ----

    pub async fn insert_code(
        &self,
        code: &str,
        kind: &str,
        duration_days: Option<i64>,
        max_devices: i64,
        note: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO codes(code,kind,duration_days,max_devices,status,note,created_at)
             VALUES (?1,?2,?3,?4,'active',?5,?6)",
            rusqlite::params![code, kind, duration_days, max_devices, note, now_secs()],
        )?;
        Ok(())
    }

    /// 激活码汇总（含绑定设备数）
    pub async fn list_codes(&self, status: Option<&str>) -> Result<Vec<CodeSummary>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT c.code,c.kind,c.duration_days,c.max_devices,c.status,c.note,c.anchor,c.created_at,
                    (SELECT COUNT(*) FROM code_devices d WHERE d.code=c.code)
             FROM codes c
             WHERE (?1 IS NULL OR c.status = ?1)
             ORDER BY c.created_at DESC",
        )?;
        let rows = stmt.query_map([status], |row| {
            Ok(CodeSummary {
                code: row.get(0)?,
                kind: row.get(1)?,
                duration_days: row.get(2)?,
                max_devices: row.get(3)?,
                status: row.get(4)?,
                note: row.get(5)?,
                anchor: row.get(6)?,
                created_at: row.get(7)?,
                devices: row.get(8)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub async fn revoke_code(&self, code: &str) -> Result<bool> {
        let conn = self.conn.lock().await;
        let n = conn.execute("UPDATE codes SET status='revoked' WHERE code=?1 AND status='active'", [code])?;
        Ok(n > 0)
    }

    pub async fn list_devices(&self, code: &str) -> Result<Vec<DeviceRow>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT device_id,device_name,platform,app_version,activated_at,last_seen
             FROM code_devices WHERE code=?1 ORDER BY activated_at",
        )?;
        let rows = stmt.query_map([code], |row| {
            Ok(DeviceRow {
                device_id: row.get(0)?,
                device_name: row.get(1)?,
                platform: row.get(2)?,
                app_version: row.get(3)?,
                activated_at: row.get(4)?,
                last_seen: row.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeSummary {
    pub code: String,
    pub kind: String,
    pub duration_days: Option<i64>,
    pub max_devices: i64,
    pub status: String,
    pub note: Option<String>,
    pub anchor: Option<i64>,
    pub created_at: i64,
    pub devices: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRow {
    pub device_id: String,
    pub device_name: Option<String>,
    pub platform: Option<String>,
    pub app_version: Option<String>,
    pub activated_at: i64,
    pub last_seen: i64,
}
