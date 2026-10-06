-- D1 schema（与原 license-server SQLite 表结构一致）
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
