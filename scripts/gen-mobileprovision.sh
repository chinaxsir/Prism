#!/usr/bin/env bash
# 为 TrollStore IPA 生成自签名 embedded.mobileprovision。
#
# 背景：
#   TrollStore 通过 CoreTrust 漏洞绕过代码签名校验，但 pkd（插件守护进程）
#   仍要求 .app / .appex 内存在 embedded.mobileprovision 才会注册网络扩展。
#   缺失时 NEVPNConnectionErrorDomain code=14（"VPN app not installed"）。
#
#   本脚本用自签证书生成 CMS 签名的 provisioning profile，结构与 Apple 官方
#   profile 一致；CoreTrust bypass 使设备接受任意签名的 profile。
#
# 用法：
#   gen-mobileprovision.sh <app-dir> <entitlements-plist> <bundle-id>
#
# 产物：
#   <app-dir>/embedded.mobileprovision
set -euo pipefail

APP_DIR="$1"
ENTS_PLIST="$2"
BUNDLE_ID="$3"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

CERT="$WORK/cert.pem"
KEY="$WORK/key.pem"
PLIST="$WORK/profile.plist"
OUT="$APP_DIR/embedded.mobileprovision"

# 1. 自签证书（10 年有效期）
openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "$KEY" -out "$CERT" -days 3650 \
  -subj "/CN=Prism TrollStore/O=TROLLTROLL/OU=TROLLTROLL/C=US" 2>/dev/null

# 2. 提取 entitlements plist 内容（取 <dict>...</dict> 作为 profile 的
#    Entitlements 字段值）
ENTS_DICT="$(plutil -convert xml1 -o - "$ENTS_PLIST" \
  | sed -n '/<dict>/,/<\/dict>/p')"

# 3. 构造 provisioning profile plist
#    关键字段：
#      - ApplicationIdentifierPrefix / TeamIdentifier: TROLLTROLL
#      - Entitlements: 与二进制内嵌 entitlements 一致
#      - TimeToLive: 3650 天
#      - UUID: 随机
UUID="$(uuidgen)"
NOW="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
EXP="$(date -u -v+3650d +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || \
       date -u -d '+3650 days' +%Y-%m-%dT%H:%M:%SZ)"

cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>AppIDName</key>
	<string>Prism</string>
	<key>ApplicationIdentifierPrefix</key>
	<array>
		<string>TROLLTROLL</string>
	</array>
	<key>CreationDate</key>
	<date>${NOW}</date>
	<key>Entitlements</key>
${ENTS_DICT}
	<key>ExpirationDate</key>
	<date>${EXP}</date>
	<key>Name</key>
	<string>Prism ${BUNDLE_ID}</string>
	<key>TeamIdentifier</key>
	<array>
		<string>TROLLTROLL</string>
	</array>
	<key>TeamName</key>
	<string>TROLLTROLL</string>
	<key>TimeToLive</key>
	<integer>3650</integer>
	<key>UUID</key>
	<string>${UUID}</string>
	<key>Version</key>
	<integer>1</integer>
</dict>
</plist>
EOF

# 4. CMS 签名（DER 输出），得到 mobileprovision
openssl cms -sign -in "$PLIST" -out "$OUT" \
  -signer "$CERT" -inkey "$KEY" \
  -outform DER -nodetach -binary 2>/dev/null

# 5. 验证产物可被解析（仅打印关键字段确认结构正确）
echo "generated: $OUT ($(du -h "$OUT" | cut -f1))"
security cms -D -i "$OUT" 2>/dev/null | plutil -extract UUID raw -o - - || true
