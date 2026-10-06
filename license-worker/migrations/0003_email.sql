-- 邮箱+授权码模式：激活码绑定邮箱（首次激活时锚定）
ALTER TABLE codes ADD COLUMN email TEXT;
