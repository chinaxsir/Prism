//! D1 数据访问层（SQL 语义与原 rusqlite 版一致：锚点订阅、设备上限、upsert）。

use serde::{Deserialize, Serialize};
use wasm_bindgen::JsValue;
use worker::D1Database;

use crate::models::DeviceInfo;

pub fn now_secs() -> i64 {
    (js_sys::Date::now() / 1000.0) as i64
}

/// D1 不接受 undefined：Option::None 必须序列化为 SQL NULL，
/// 因此显式开启 serialize_missing_as_null（默认 to_value 会输出 undefined）。
fn js<T: Serialize>(v: T) -> JsValue {
    let ser =
        serde_wasm_bindgen::Serializer::new().serialize_missing_as_null(true);
    v.serialize(&ser).unwrap_or(JsValue::NULL)
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
    /// 邮箱绑定（首次激活锚定）
    pub email: Option<String>,
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
    pub email: Option<String>,
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlacklistRow {
    pub device_id: String,
    pub reason: Option<String>,
    pub created_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsTotals {
    pub codes_total: i64,
    pub codes_active: i64,
    pub devices_total: i64,
    pub store_purchases: i64,
    pub blacklist: i64,
}

/// 存量 / 到期分布 / 渠道明细
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsBreakdown {
    /// 未发出（active 且零设备绑定）
    pub code_unused: i64,
    /// 使用中（active 且至少 1 台设备）
    pub code_in_use: i64,
    /// 已停用/吊销
    pub code_revoked: i64,
    /// 订阅码：已到期（已锚定且超过到期时间）
    pub sub_expired: i64,
    /// 7 天内到期（不含已到期）
    pub sub_exp_7d: i64,
    /// 8–30 天内到期
    pub sub_exp_30d: i64,
    /// 30 天后到期
    pub sub_active: i64,
    /// 商店渠道去重设备数
    pub store_devices: i64,
    /// 商店渠道有效购买（买断或订阅未到期）
    pub store_active: i64,
}

#[derive(Serialize, Deserialize)]
pub struct DayCount {
    pub day: i64,
    pub n: i64,
}

impl Db {
    pub fn new(d1: D1Database) -> Self {
        Db { d1 }
    }

    pub async fn get_code(&self, code: &str) -> Result<Option<CodeRecord>, String> {
        self.d1
            .prepare(
                "SELECT code,kind,duration_days,max_devices,status,anchor,email,note,created_at
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

    /// 绑定邮箱（仅首次，后续激活由调用方校验一致性）
    pub async fn bind_email(&self, code: &str, email: &str) -> Result<(), String> {
        self.d1
            .prepare("UPDATE codes SET email = ?1 WHERE code = ?2 AND email IS NULL")
            .bind(&[js(email), js(code)])
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
        email: Option<&str>,
    ) -> Result<(), String> {
        self.d1
            .prepare(
                "INSERT INTO codes(code,kind,duration_days,max_devices,status,note,email,created_at)
                 VALUES (?1,?2,?3,?4,'active',?5,?6,?7)",
            )
            .bind(&[
                js(code),
                js(kind),
                js(duration_days),
                js(max_devices),
                js(note),
                js(email),
                js(now_secs()),
            ])
            .map_err(|e| e.to_string())?
            .run()
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub async fn list_codes(
        &self,
        status: Option<&str>,
        q: Option<&str>,
    ) -> Result<Vec<CodeSummary>, String> {
        let like = q.map(|s| format!("%{}%", s.trim()));
        let result = self
            .d1
            .prepare(
                "SELECT c.code AS code,c.kind AS kind,c.duration_days AS durationDays,
                        c.max_devices AS maxDevices,c.status AS status,c.note AS note,
                        c.email AS email,c.anchor AS anchor,c.created_at AS createdAt,
                        (SELECT COUNT(*) FROM code_devices d WHERE d.code=c.code) AS devices
                 FROM codes c
                 WHERE (?1 IS NULL OR c.status = ?1)
                   AND (?2 IS NULL OR c.code LIKE ?2 OR c.note LIKE ?2 OR c.email LIKE ?2)
                 ORDER BY c.created_at DESC",
            )
            .bind(&[js(status), js(like.as_deref())])
            .map_err(|e| e.to_string())?
            .all()
            .await
            .map_err(|e| e.to_string())?;
        result.results::<CodeSummary>().map_err(|e| e.to_string())
    }

    /// 管理端更新激活码（任一字段为 None 表示不修改）。
    /// - `extend_days`：订阅码在现有 duration_days 上顺延（不受 duration_days 参数影响）
    /// - `note`：双 Option，None=不改；Some(None)=清空
    /// - `email`：None=不改；Some("")=解绑；Some(v)=改绑
    /// 返回 false 表示码不存在
    #[allow(clippy::too_many_arguments)]
    pub async fn update_code(
        &self,
        code: &str,
        max_devices: Option<i64>,
        duration_days: Option<i64>,
        extend_days: Option<i64>,
        status: Option<&str>,
        note: Option<Option<&str>>,
        email: Option<&str>,
    ) -> Result<bool, String> {
        let Some(mut rec) = self.get_code(code).await? else {
            return Ok(false);
        };
        if let Some(m) = max_devices {
            rec.max_devices = m.clamp(1, 100);
        }
        if let Some(d) = duration_days {
            rec.duration_days = Some(d);
        }
        if let Some(add) = extend_days {
            rec.duration_days = Some(rec.duration_days.unwrap_or(365) + add);
        }
        if let Some(s) = status {
            rec.status = s.to_string();
        }
        let new_note = match note {
            None => rec.note.clone(),
            Some(None) => None,
            Some(Some(v)) => Some(v.to_string()),
        };
        let new_email: Option<String> = match email {
            None => rec.email.clone(),
            Some("") => None,
            Some(v) => Some(v.trim().to_lowercase()),
        };
        self.d1
            .prepare(
                "UPDATE codes SET max_devices=?2,duration_days=?3,status=?4,note=?5,email=?6
                 WHERE code=?1",
            )
            .bind(&[
                js(code),
                js(rec.max_devices),
                js(rec.duration_days),
                js(rec.status),
                js(new_note.as_deref()),
                js(new_email.as_deref()),
            ])
            .map_err(|e| e.to_string())?
            .run()
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())?;
        Ok(true)
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

    // ---- 黑名单 ----

    /// 判断设备是否在黑名单
    pub async fn is_device_blacklisted(&self, device_id: &str) -> Result<bool, String> {
        let row = self
            .d1
            .prepare("SELECT COUNT(*) AS n FROM blacklist WHERE device_id=?1")
            .bind(&[js(device_id)])
            .map_err(|e| e.to_string())?
            .first::<CountRow>(None)
            .await
            .map_err(|e| e.to_string())?;
        Ok(row.map(|r| r.n).unwrap_or(0) > 0)
    }

    pub async fn blacklist_add(&self, device_id: &str, reason: Option<&str>) -> Result<(), String> {
        self.d1
            .prepare(
                "INSERT INTO blacklist(device_id,reason,created_at) VALUES (?1,?2,?3)
                 ON CONFLICT(device_id) DO UPDATE SET reason=excluded.reason,created_at=excluded.created_at",
            )
            .bind(&[js(device_id), js(reason), js(now_secs())])
            .map_err(|e| e.to_string())?
            .run()
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub async fn blacklist_remove(&self, device_id: &str) -> Result<bool, String> {
        let res = self
            .d1
            .prepare("DELETE FROM blacklist WHERE device_id=?1")
            .bind(&[js(device_id)])
            .map_err(|e| e.to_string())?
            .run()
            .await
            .map_err(|e| e.to_string())?;
        let changes = res
            .meta()
            .ok()
            .flatten()
            .and_then(|m| m.changes)
            .unwrap_or(0);
        Ok(changes > 0)
    }

    pub async fn blacklist_list(&self) -> Result<Vec<BlacklistRow>, String> {
        let result = self
            .d1
            .prepare(
                "SELECT device_id AS deviceId,reason AS reason,created_at AS createdAt
                 FROM blacklist ORDER BY created_at DESC",
            )
            .bind(&[])
            .map_err(|e| e.to_string())?
            .all()
            .await
            .map_err(|e| e.to_string())?;
        result.results::<BlacklistRow>().map_err(|e| e.to_string())
    }

    // ---- 统计 ----

    /// 汇总计数：激活码总数/在用、设备总数、商店购买数、黑名单数
    pub async fn stats_totals(&self) -> Result<StatsTotals, String> {
        async fn one(d1: &D1Database, sql: &str) -> Result<i64, String> {
            let row = d1
                .prepare(sql)
                .bind(&[])
                .map_err(|e| e.to_string())?
                .first::<CountRow>(None)
                .await
                .map_err(|e| e.to_string())?;
            Ok(row.map(|r| r.n).unwrap_or(0))
        }
        Ok(StatsTotals {
            codes_total: one(&self.d1, "SELECT COUNT(*) AS n FROM codes").await?,
            codes_active: one(&self.d1, "SELECT COUNT(*) AS n FROM codes WHERE status='active'").await?,
            devices_total: one(&self.d1, "SELECT COUNT(*) AS n FROM code_devices").await?,
            store_purchases: one(&self.d1, "SELECT COUNT(*) AS n FROM store_purchases").await?,
            blacklist: one(&self.d1, "SELECT COUNT(*) AS n FROM blacklist").await?,
        })
    }

    /// 存量 / 到期分布 / 渠道明细
    pub async fn stats_breakdown(&self) -> Result<StatsBreakdown, String> {
        async fn one(d1: &D1Database, sql: &str, binds: Vec<JsValue>) -> Result<i64, String> {
            let row = d1
                .prepare(sql)
                .bind(&binds)
                .map_err(|e| e.to_string())?
                .first::<CountRow>(None)
                .await
                .map_err(|e| e.to_string())?;
            Ok(row.map(|r| r.n).unwrap_or(0))
        }
        let now = now_secs();
        let d7 = now + 7 * 86400;
        let d30 = now + 30 * 86400;
        // 订阅到期时间列（仅已锚定的订阅码参与）
        let exp = "anchor + COALESCE(duration_days,365)*86400";
        let sub_where = format!(
            "status='active' AND kind='subscription' AND anchor IS NOT NULL AND {exp}"
        );
        let d1 = &self.d1;
        Ok(StatsBreakdown {
            code_unused: one(
                d1,
                "SELECT COUNT(*) AS n FROM codes c WHERE c.status='active'
                 AND NOT EXISTS (SELECT 1 FROM code_devices d WHERE d.code=c.code)",
                vec![],
            )
            .await?,
            code_in_use: one(
                d1,
                "SELECT COUNT(*) AS n FROM codes c WHERE c.status='active'
                 AND EXISTS (SELECT 1 FROM code_devices d WHERE d.code=c.code)",
                vec![],
            )
            .await?,
            code_revoked: one(d1, "SELECT COUNT(*) AS n FROM codes WHERE status!='active'", vec![])
                .await?,
            sub_expired: one(
                d1,
                &format!("SELECT COUNT(*) AS n FROM codes WHERE {sub_where} <= ?1"),
                vec![js(now)],
            )
            .await?,
            sub_exp_7d: one(
                d1,
                &format!("SELECT COUNT(*) AS n FROM codes WHERE {sub_where} > ?1 AND {exp} <= ?2"),
                vec![js(now), js(d7)],
            )
            .await?,
            sub_exp_30d: one(
                d1,
                &format!("SELECT COUNT(*) AS n FROM codes WHERE {sub_where} > ?1 AND {exp} <= ?2"),
                vec![js(d7), js(d30)],
            )
            .await?,
            sub_active: one(
                d1,
                &format!("SELECT COUNT(*) AS n FROM codes WHERE {sub_where} > ?1"),
                vec![js(d30)],
            )
            .await?,
            store_devices: one(
                d1,
                "SELECT COUNT(DISTINCT device_id) AS n FROM store_devices",
                vec![],
            )
            .await?,
            store_active: one(
                d1,
                "SELECT COUNT(*) AS n FROM store_purchases
                 WHERE expires_at IS NULL OR expires_at > ?1",
                vec![js(now)],
            )
            .await?,
        })
    }

    pub async fn activations_by_day(&self, days: i64) -> Result<Vec<DayCount>, String> {
        let since = now_secs() - days * 86400;
        let result = self
            .d1
            .prepare(
                "SELECT (activated_at/86400)*86400 AS day, COUNT(*) AS n
                 FROM code_devices WHERE activated_at >= ?1
                 GROUP BY day ORDER BY day",
            )
            .bind(&[js(since)])
            .map_err(|e| e.to_string())?
            .all()
            .await
            .map_err(|e| e.to_string())?;
        result.results::<DayCount>().map_err(|e| e.to_string())
    }

    /// 彻底删除激活码及其设备绑定
    pub async fn delete_code(&self, code: &str) -> Result<bool, String> {
        if self.get_code(code).await?.is_none() {
            return Ok(false);
        }
        self.d1
            .prepare("DELETE FROM code_devices WHERE code=?1")
            .bind(&[js(code)])
            .map_err(|e| e.to_string())?
            .run()
            .await
            .map_err(|e| e.to_string())?;
        self.d1
            .prepare("DELETE FROM codes WHERE code=?1")
            .bind(&[js(code)])
            .map_err(|e| e.to_string())?
            .run()
            .await
            .map_err(|e| e.to_string())?;
        Ok(true)
    }
}
