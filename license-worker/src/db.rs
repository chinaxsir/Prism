//! D1 数据访问层（SQL 语义与原 rusqlite 版一致：锚点订阅、设备上限、upsert）。

use serde::{Deserialize, Serialize};
use serde_wasm_bindgen::to_value as js_value;
use wasm_bindgen::JsValue;
use worker::D1Database;

use crate::models::DeviceInfo;

pub fn now_secs() -> i64 {
    (js_sys::Date::now() / 1000.0) as i64
}

fn js<T: serde::Serialize>(v: T) -> JsValue {
    js_value(&v).unwrap_or(JsValue::NULL)
}

pub struct Db {
    d1: D1Database,
}

#[derive(Deserialize)]
#[allow(dead_code)] // code/created_at 等列保留以映射完整行
pub struct CodeRecord {
    pub code: String,
    pub kind: String,
    pub duration_days: Option<i64>,
    pub max_devices: i64,
    pub status: String,
    pub anchor: Option<i64>,
    #[allow(dead_code)]
    pub note: Option<String>,
    pub created_at: i64,
}

#[derive(Deserialize)]
struct StoreRow {
    product_id: String,
    kind: String,
    expires_at: Option<i64>,
}

#[derive(Deserialize)]
struct CountRow {
    n: i64,
}

#[derive(Serialize, Deserialize)]
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRow {
    pub device_id: String,
    pub device_name: Option<String>,
    pub platform: Option<String>,
    pub app_version: Option<String>,
    pub activated_at: i64,
    pub last_seen: i64,
}

impl Db {
    pub fn new(d1: D1Database) -> Self {
        Db { d1 }
    }

    pub async fn get_code(&self, code: &str) -> Result<Option<CodeRecord>, String> {
        self.d1
            .prepare(
                "SELECT code,kind,duration_days,max_devices,status,anchor,note,created_at
                 FROM codes WHERE code = ?1",
            )
            .bind(&[js(code)])
            .map_err(|e| e.to_string())?
            .first::<CodeRecord>(None)
            .await
            .map_err(|e| e.to_string())
    }

    /// 设置订阅锚点（仅首次）
    pub async fn set_anchor(&self, code: &str, anchor: i64) -> Result<(), String> {
        self.d1
            .prepare("UPDATE codes SET anchor = ?1 WHERE code = ?2 AND anchor IS NULL")
            .bind(&[js(anchor), js(code)])
            .map_err(|e| e.to_string())?
            .run()
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub async fn device_count(&self, code: &str) -> Result<i64, String> {
        let row = self
            .d1
            .prepare("SELECT COUNT(*) AS n FROM code_devices WHERE code = ?1")
            .bind(&[js(code)])
            .map_err(|e| e.to_string())?
            .first::<CountRow>(None)
            .await
            .map_err(|e| e.to_string())?;
        Ok(row.map(|r| r.n).unwrap_or(0))
    }

    pub async fn is_device_bound(&self, code: &str, device_id: &str) -> Result<bool, String> {
        let row = self
            .d1
            .prepare("SELECT COUNT(*) AS n FROM code_devices WHERE code=?1 AND device_id=?2")
            .bind(&[js(code), js(device_id)])
            .map_err(|e| e.to_string())?
            .first::<CountRow>(None)
            .await
            .map_err(|e| e.to_string())?;
        Ok(row.map(|r| r.n).unwrap_or(0) > 0)
    }

    /// 返回 true 表示新插入设备，false 表示已存在（仅刷新）
    pub async fn upsert_code_device(&self, code: &str, d: &DeviceInfo) -> Result<bool, String> {
        let exists = self.is_device_bound(code, &d.id).await?;
        let now = now_secs();
        if exists {
            self.d1
                .prepare(
                    "UPDATE code_devices SET last_seen=?1, device_name=?2, platform=?3,
                       app_version=?4 WHERE code=?5 AND device_id=?6",
                )
                .bind(&[
                    js(now),
                    js(&d.name),
                    js(&d.platform),
                    js(&d.app_version),
                    js(code),
                    js(&d.id),
                ])
                .map_err(|e| e.to_string())?
                .run()
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())?;
            Ok(false)
        } else {
            self.d1
                .prepare(
                    "INSERT INTO code_devices
                       (code,device_id,device_name,platform,app_version,activated_at,last_seen)
                     VALUES (?1,?2,?3,?4,?5,?6,?6)",
                )
                .bind(&[
                    js(code),
                    js(&d.id),
                    js(&d.name),
                    js(&d.platform),
                    js(&d.app_version),
                    js(now),
                ])
                .map_err(|e| e.to_string())?
                .run()
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())?;
            Ok(true)
        }
    }

    pub async fn unbind_device(&self, code: &str, device_id: &str) -> Result<bool, String> {
        let existed = self.is_device_bound(code, device_id).await?;
        if !existed {
            return Ok(false);
        }
        self.d1
            .prepare("DELETE FROM code_devices WHERE code=?1 AND device_id=?2")
            .bind(&[js(code), js(device_id)])
            .map_err(|e| e.to_string())?
            .run()
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())?;
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_store_purchase(
        &self,
        store: &str,
        original_id: &str,
        product_id: &str,
        kind: &str,
        expires_at: Option<i64>,
        raw: &str,
    ) -> Result<(), String> {
        let now = now_secs();
        self.d1
            .prepare(
                "INSERT INTO store_purchases
                   (store,original_id,product_id,kind,expires_at,raw,first_seen,last_seen)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?7)
                 ON CONFLICT(store,original_id) DO UPDATE SET
                   product_id=excluded.product_id, kind=excluded.kind,
                   expires_at=excluded.expires_at, raw=excluded.raw, last_seen=?7",
            )
            .bind(&[
                js(store),
                js(original_id),
                js(product_id),
                js(kind),
                js(expires_at),
                js(raw),
                js(now),
            ])
            .map_err(|e| e.to_string())?
            .run()
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub async fn get_store_purchase(
        &self,
        store: &str,
        original_id: &str,
    ) -> Result<Option<(String, String, Option<i64>)>, String> {
        let row = self
            .d1
            .prepare(
                "SELECT product_id,kind,expires_at FROM store_purchases
                 WHERE store=?1 AND original_id=?2",
            )
            .bind(&[js(store), js(original_id)])
            .map_err(|e| e.to_string())?
            .first::<StoreRow>(None)
            .await
            .map_err(|e| e.to_string())?;
        Ok(row.map(|r| (r.product_id, r.kind, r.expires_at)))
    }

    pub async fn upsert_store_device(
        &self,
        store: &str,
        original_id: &str,
        d: &DeviceInfo,
    ) -> Result<(), String> {
        let now = now_secs();
        self.d1
            .prepare(
                "INSERT INTO store_devices
                   (store,original_id,device_id,device_name,platform,app_version,last_seen)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(store,original_id,device_id) DO UPDATE SET
                   device_name=excluded.device_name, platform=excluded.platform,
                   app_version=excluded.app_version, last_seen=excluded.last_seen",
            )
            .bind(&[
                js(store),
                js(original_id),
                js(&d.id),
                js(&d.name),
                js(&d.platform),
                js(&d.app_version),
                js(now),
            ])
            .map_err(|e| e.to_string())?
            .run()
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    // ---- 管理接口 ----

    pub async fn insert_code(
        &self,
        code: &str,
        kind: &str,
        duration_days: Option<i64>,
        max_devices: i64,
        note: Option<&str>,
    ) -> Result<(), String> {
        self.d1
            .prepare(
                "INSERT INTO codes(code,kind,duration_days,max_devices,status,note,created_at)
                 VALUES (?1,?2,?3,?4,'active',?5,?6)",
            )
            .bind(&[
                js(code),
                js(kind),
                js(duration_days),
                js(max_devices),
                js(note),
                js(now_secs()),
            ])
            .map_err(|e| e.to_string())?
            .run()
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub async fn list_codes(&self, status: Option<&str>) -> Result<Vec<CodeSummary>, String> {
        let result = self
            .d1
            .prepare(
                "SELECT c.code AS code,c.kind AS kind,c.duration_days AS durationDays,
                        c.max_devices AS maxDevices,c.status AS status,c.note AS note,
                        c.anchor AS anchor,c.created_at AS createdAt,
                        (SELECT COUNT(*) FROM code_devices d WHERE d.code=c.code) AS devices
                 FROM codes c
                 WHERE (?1 IS NULL OR c.status = ?1)
                 ORDER BY c.created_at DESC",
            )
            .bind(&[js(status)])
            .map_err(|e| e.to_string())?
            .all()
            .await
            .map_err(|e| e.to_string())?;
        result.results::<CodeSummary>().map_err(|e| e.to_string())
    }

    pub async fn revoke_code(&self, code: &str) -> Result<bool, String> {
        let record = self.get_code(code).await?;
        let Some(record) = record else {
            return Ok(false);
        };
        if record.status != "active" {
            return Ok(false);
        }
        self.d1
            .prepare("UPDATE codes SET status='revoked' WHERE code=?1")
            .bind(&[js(code)])
            .map_err(|e| e.to_string())?
            .run()
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())?;
        Ok(true)
    }

    pub async fn list_devices(&self, code: &str) -> Result<Vec<DeviceRow>, String> {
        let result = self
            .d1
            .prepare(
                "SELECT device_id AS deviceId,device_name AS deviceName,platform AS platform,
                        app_version AS appVersion,activated_at AS activatedAt,
                        last_seen AS lastSeen
                 FROM code_devices WHERE code=?1 ORDER BY activated_at",
            )
            .bind(&[js(code)])
            .map_err(|e| e.to_string())?
            .all()
            .await
            .map_err(|e| e.to_string())?;
        result.results::<DeviceRow>().map_err(|e| e.to_string())
    }
}
