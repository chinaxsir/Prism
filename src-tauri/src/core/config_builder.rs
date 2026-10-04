//! 配置构建器：Clash YAML 订阅 → sing-box runtime JSON
//!
//! 输入支持：
//! 1. Clash/Mihomo 格式 YAML（最常见的订阅格式）
//! 2. 已是 sing-box JSON 的配置（原样透传）
//!
//! 输出 sing-box 配置包含：mixed 入站、可选 tun 入站、内置出站、
//! 节点出站、策略组、DNS、路由规则、clash_api 实验功能。
//!
//! 无法无损转换的项会写入 warnings，调用方应记录日志而不是直接失败。

use std::collections::HashSet;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use super::state::{RunMode, UserSettings};

const DEFAULT_TEST_URL: &str = "http://www.gstatic.com/generate_204";
const TUN_TAG: &str = "tun-in";
const MIXED_TAG: &str = "mixed-in";

pub struct BuiltConfig {
    pub config: Value,
    /// 转换过程中的降级/跳过提示（不致命）
    pub warnings: Vec<String>,
}

// ---------------- 输入：Clash 订阅结构 ----------------

#[derive(Deserialize, Default)]
struct ClashProfile {
    #[serde(default)]
    proxies: Vec<ClashNode>,
    #[serde(default, rename = "proxy-groups")]
    proxy_groups: Vec<ClashGroup>,
    #[serde(default)]
    rules: Vec<String>,
}

#[derive(Deserialize)]
struct ClashNode {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    server: String,
    port: u16,

    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    uuid: Option<String>,
    /// SS 加密方式；VMess 中为 security
    #[serde(default)]
    cipher: Option<String>,
    #[serde(default)]
    alter_id: Option<i64>,
    #[serde(default)]
    network: Option<String>,
    #[serde(default)]
    tls: Option<bool>,
    #[serde(default)]
    sni: Option<String>,
    #[serde(default)]
    skip_cert_verify: Option<bool>,
    #[serde(default)]
    alpn: Option<Vec<String>>,
    #[serde(default, rename = "ws-opts")]
    ws_opts: Option<WsOpts>,
    #[serde(default, rename = "grpc-opts")]
    grpc_opts: Option<GrpcOpts>,
    #[serde(default)]
    flow: Option<String>,
    #[serde(default, rename = "congestion-controller")]
    congestion_control: Option<String>,
    /// SS 插件 obfs / v2ray-plugin
    #[serde(default)]
    plugin: Option<String>,
    #[serde(default, rename = "plugin-opts")]
    plugin_opts: Option<Value>,
}

#[derive(Deserialize)]
struct WsOpts {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    headers: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Deserialize)]
struct GrpcOpts {
    #[serde(default, rename = "grpc-service-name")]
    service_name: Option<String>,
}

#[derive(Deserialize)]
struct ClashGroup {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    proxies: Vec<String>,
    #[serde(default)]
    url: Option<String>,
    /// Clash 间隔单位为秒
    #[serde(default)]
    interval: Option<i64>,
    #[serde(default)]
    strategy: Option<String>,
}

// ---------------- 入口 ----------------

pub fn build(
    profile_text: &str,
    mode: RunMode,
    settings: &UserSettings,
    work_dir: &Path,
    api_addr: &str,
    api_secret: &str,
) -> Result<BuiltConfig> {
    let trimmed = profile_text.trim();

    // 已是 sing-box JSON：原样透传（调用方自行保证字段完整）
    if trimmed.starts_with('{') {
        let config: Value = serde_json::from_str(trimmed)
            .context("profile looks like JSON but failed to parse")?;
        return Ok(BuiltConfig {
            config,
            warnings: vec!["sing-box JSON 配置原样透传，未做模式/端口合并".into()],
        });
    }

    let profile: ClashProfile = serde_yaml::from_str(trimmed)
        .context("无法解析为 Clash YAML 订阅（请检查订阅格式）")?;

    let mut warnings = Vec::new();
    let mut known_tags: HashSet<String> =
        ["direct", "block", "dns-out"].map(str::to_string).into_iter().collect();

    // 1. 节点出站
    let mut outbounds = vec![
        json!({ "type": "direct", "tag": "direct" }),
        json!({ "type": "block", "tag": "block" }),
        json!({ "type": "dns", "tag": "dns-out" }),
    ];

    for node in &profile.proxies {
        let tag = node.name.clone();
        if !known_tags.insert(tag.clone()) {
            warnings.push(format!("节点/组标签重复，已跳过：{}", tag));
            continue;
        }

        match node_to_outbound(node) {
            Some(outbound) => outbounds.push(outbound),
            None => warnings.push(format!(
                "不支持的节点类型，已跳过 '{}' ({})",
                node.name, node.kind
            )),
        }
    }

    // 2. 策略组出站（组可引用组，最后统一校验）
    for group in &profile.proxy_groups {
        let tag = group.name.clone();
        if !known_tags.insert(tag.clone()) {
            warnings.push(format!("节点/组标签重复，已跳过：{}", tag));
            continue;
        }
        outbounds.push(group_to_outbound(group, &mut warnings));
    }

    // 3. 规则 → sing-box route.rules
    let mut route_rules: Vec<Value> = Vec::new();
    let mut final_tag: Option<String> = None;

    for line in &profile.rules {
        let parts: Vec<&str> = line.split(',').map(str::trim).collect();
        if parts.is_empty() || parts[0].is_empty() {
            continue;
        }

        let kind = parts[0].to_ascii_uppercase();

        // MATCH / FINAL 决定 route.final
        if kind == "MATCH" || kind == "FINAL" {
            final_tag = Some(map_ref(parts.get(1).copied().unwrap_or("DIRECT")));
            continue;
        }

        let Some(payload) = parts.get(1).copied() else {
            warnings.push(format!("规则缺少参数，已跳过：{}", line));
            continue;
        };
        let policy = parts.get(2).copied().unwrap_or("DIRECT");
        let outbound = map_ref(policy);

        let rule = match kind.as_str() {
            "DOMAIN" => Some(json!({ "domain": [payload], "outbound": outbound })),
            "DOMAIN-SUFFIX" => Some(json!({ "domain_suffix": [payload], "outbound": outbound })),
            "DOMAIN-KEYWORD" => Some(json!({ "domain_keyword": [payload], "outbound": outbound })),
            "IP-CIDR" | "IP-CIDR6" => Some(json!({ "ip_cidr": [payload], "outbound": outbound })),
            "GEOIP" => Some(json!({ "geoip": payload, "outbound": outbound })),
            "GEOSITE" => Some(json!({ "geosite": payload, "outbound": outbound })),
            "PROCESS-NAME" => Some(json!({ "process_name": [payload], "outbound": outbound })),
            "PROCESS-PATH" => Some(json!({ "process_path": [payload], "outbound": outbound })),
            "SRC-IP-CIDR" => Some(json!({ "source_ip_cidr": [payload], "outbound": outbound })),
            "SRC-PORT" => Some(json!({ "source_port": [payload.parse::<u32>().unwrap_or(0)], "outbound": outbound })),
            "DST-PORT" => Some(json!({ "port": [payload.parse::<u32>().unwrap_or(0)], "outbound": outbound })),
            "NETWORK" => Some(json!({ "network": payload, "outbound": outbound })),
            // RULE-SET 依赖 rule-providers 元信息，无法从纯 Clash 配置无损转换
            "RULE-SET" => {
                warnings.push(format!(
                    "RULE-SET ({}) 需要规则集元信息，已跳过；请改用 GEOSITE 或 sing-box rule_set",
                    payload
                ));
                None
            }
            "AND" | "OR" | "NOT" | "SUB-RULE" => {
                warnings.push(format!("组合规则 {} 暂不支持转换，已跳过", kind));
                None
            }
            other => {
                warnings.push(format!("未知规则类型 {}，已跳过：{}", other, line));
                None
            }
        };

        if let Some(rule) = rule {
            route_rules.push(rule);
        }
    }

    // 未显式 MATCH：优先首个策略组，否则 direct
    let final_tag = final_tag.unwrap_or_else(|| {
        profile
            .proxy_groups
            .first()
            .map(|g| g.name.clone())
            .unwrap_or_else(|| "direct".into())
    });

    // 校验策略组引用，剔除悬空目标
    // 注意：必须用 get_mut 探测，直接 outbound["outbounds"] 可变索引会给
    // 普通节点出站插入 "outbounds": null，导致 sing-box 报 unknown field
    for outbound in outbounds.iter_mut().skip(3) {
        let tag = outbound["tag"].as_str().unwrap_or_default().to_string();
        let list = outbound.get_mut("outbounds").and_then(Value::as_array_mut);
        if let Some(list) = list {
            let before = list.len();
            list.retain(|item| {
                let t = item.as_str().unwrap_or_default();
                known_tags.contains(t) || matches!(t, "direct" | "block")
            });
            if list.len() != before {
                warnings.push(format!("策略组 '{}' 中存在无效引用，已剔除", tag));
            }
        }
    }

    // 4. 入站
    let mut inbounds = vec![json!({
        "type": "mixed",
        "tag": MIXED_TAG,
        "listen": if settings.allow_lan { "0.0.0.0" } else { "127.0.0.1" },
        "listen_port": settings.mixed_port,
    })];

    if mode == RunMode::Tun {
        inbounds.push(json!({
            "type": "tun",
            "tag": TUN_TAG,
            "interface_name": "prism-tun",
            "inet4_address": "198.18.0.1/30",
            "mtu": 9000,
            "auto_route": true,
            "strict_route": true,
            "stack": "gvisor",
        }));
    }

    // 5. DNS（1.11 legacy 格式：servers 用 address；1.12 的 type/server 尚未支持）
    let dns = json!({
        "servers": [
            { "tag": "dns-remote", "address": "8.8.8.8" },
            { "tag": "dns-local", "address": "223.5.5.5", "detour": "direct" }
        ],
        "rules": [
            { "domain_suffix": [".cn"], "server": "dns-local" }
        ],
        "final": "dns-remote",
        "strategy": "prefer_ipv4"
    });

    // 6. clash_api（驱动前端实时数据；地址动态选择，secret 随机生成）
    // 注意：store_selected 等字段在 1.8+ 已从 clash_api 移除，写了会 FATAL
    let cache_path = work_dir.join("cache.db");
    let mut clash_api = json!({
        "external_controller": api_addr
    });
    if !api_secret.is_empty() {
        clash_api["secret"] = json!(api_secret);
    }
    let experimental = json!({
        "clash_api": clash_api,
        "cache_file": {
            "enabled": true,
            "path": cache_path.to_string_lossy()
        }
    });

    let config = json!({
        "log": { "level": "info", "timestamp": true },
        "dns": dns,
        "inbounds": inbounds,
        "outbounds": outbounds,
        "route": {
            "rules": route_rules,
            "final": final_tag,
            "auto_detect_interface": true
        },
        "experimental": experimental
    });

    if profile.proxies.is_empty() && profile.proxy_groups.is_empty() {
        bail!("订阅中没有任何节点或策略组");
    }

    Ok(BuiltConfig { config, warnings })
}

// ---------------- 节点转换 ----------------

fn node_to_outbound(node: &ClashNode) -> Option<Value> {
    let base = json!({
        "tag": node.name,
        "server": node.server,
        "server_port": node.port,
    });

    let mut outbound = match node.kind.as_str() {
        "ss" | "shadowsocks" => {
            let mut o = json!({
                "type": "shadowsocks",
                "method": node.cipher.clone().unwrap_or_else(|| "aes-256-gcm".into()),
                "password": node.password.clone().unwrap_or_default(),
            });
            merge(&mut o, &base);

            // SS 简单插件映射（obfs）
            if let Some(plugin) = &node.plugin
                && plugin.starts_with("obfs")
            {
                let mode = node
                    .plugin_opts
                    .as_ref()
                    .and_then(|v| v.get("mode"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("http");
                let host = node
                    .plugin_opts
                    .as_ref()
                    .and_then(|v| v.get("host"))
                    .and_then(|v| v.as_str());
                let plugin = json!({
                    "type": "obfs",
                    "mode": mode,
                    "host": host
                });
                o["plugin"] = plugin;
            }
            o
        }

        "vmess" => {
            let mut o = json!({
                "type": "vmess",
                "uuid": node.uuid.clone().unwrap_or_default(),
                "security": node.cipher.clone().unwrap_or_else(|| "auto".into()),
                "alter_id": node.alter_id.unwrap_or(0),
            });
            merge(&mut o, &base);
            if let Some(tls) = build_tls(node) {
                o["tls"] = tls;
            }
            if let Some(tp) = build_transport(node) {
                o["transport"] = tp;
            }
            o
        }

        "vless" => {
            let mut o = json!({
                "type": "vless",
                "uuid": node.uuid.clone().unwrap_or_default(),
            });
            if let Some(flow) = &node.flow {
                o["flow"] = json!(flow);
            }
            merge(&mut o, &base);
            if let Some(tls) = build_tls(node) {
                o["tls"] = tls;
            }
            if let Some(tp) = build_transport(node) {
                o["transport"] = tp;
            }
            o
        }

        "trojan" => {
            let mut o = json!({
                "type": "trojan",
                "password": node.password.clone().unwrap_or_default(),
            });
            merge(&mut o, &base);
            // Trojan 始终 TLS
            o["tls"] = json!({
                "enabled": true,
                "server_name": node.sni.clone().unwrap_or_else(|| node.server.clone()),
                "insecure": node.skip_cert_verify.unwrap_or(false),
                "alpn": node.alpn.clone().unwrap_or_default(),
            });
            if let Some(tp) = build_transport(node) {
                o["transport"] = tp;
            }
            o
        }

        "tuic" => {
            let mut o = json!({
                "type": "tuic",
                "uuid": node.uuid.clone().unwrap_or_default(),
                "password": node.password.clone().unwrap_or_default(),
                "congestion_control": node.congestion_control.clone().unwrap_or_else(|| "bbr".into()),
            });
            merge(&mut o, &base);
            o["tls"] = json!({
                "enabled": true,
                "server_name": node.sni.clone().unwrap_or_else(|| node.server.clone()),
                "insecure": node.skip_cert_verify.unwrap_or(false),
                "alpn": node.alpn.clone().unwrap_or_default(),
            });
            o
        }

        "hysteria2" | "hy2" => {
            let mut o = json!({
                "type": "hysteria2",
                "password": node.password.clone().unwrap_or_default(),
            });
            merge(&mut o, &base);
            o["tls"] = json!({
                "enabled": true,
                "server_name": node.sni.clone().unwrap_or_else(|| node.server.clone()),
                "insecure": node.skip_cert_verify.unwrap_or(false),
                "alpn": node.alpn.clone().unwrap_or_default(),
            });
            o
        }

        "hysteria" => {
            let mut o = json!({
                "type": "hysteria",
                "auth_str": node.password.clone().unwrap_or_default(),
            });
            merge(&mut o, &base);
            o["tls"] = json!({
                "enabled": true,
                "server_name": node.sni.clone().unwrap_or_else(|| node.server.clone()),
                "insecure": node.skip_cert_verify.unwrap_or(false),
            });
            o
        }

        _ => return None,
    };

    // 移除值为 null 的可选字段，保持输出干净
    strip_nulls(&mut outbound);
    Some(outbound)
}

fn build_tls(node: &ClashNode) -> Option<Value> {
    if !node.tls.unwrap_or(false) {
        return None;
    }
    Some(json!({
        "enabled": true,
        "server_name": node.sni.clone().unwrap_or_else(|| node.server.clone()),
        "insecure": node.skip_cert_verify.unwrap_or(false),
        "alpn": node.alpn.clone().unwrap_or_default(),
    }))
}

fn build_transport(node: &ClashNode) -> Option<Value> {
    match node.network.as_deref() {
        Some("ws") => {
            let mut transport = json!({
                "type": "ws",
                "path": node.ws_opts.as_ref().and_then(|w| w.path.clone()).unwrap_or_else(|| "/".into())
            });
            if let Some(headers) = node.ws_opts.as_ref().and_then(|w| w.headers.clone()) {
                transport["headers"] = json!(headers);
            }
            Some(transport)
        }
        Some("grpc") => Some(json!({
            "type": "grpc",
            "service_name": node.grpc_opts.as_ref().and_then(|g| g.service_name.clone()).unwrap_or_default()
        })),
        _ => None,
    }
}

// ---------------- 策略组转换 ----------------

fn group_to_outbound(group: &ClashGroup, warnings: &mut Vec<String>) -> Value {
    let refs: Vec<Value> = group.proxies.iter().map(|r| json!(map_ref(r))).collect();
    let interval = format!("{}s", group.interval.unwrap_or(300));
    let test_url = group.url.clone().unwrap_or_else(|| DEFAULT_TEST_URL.into());

    match group.kind.as_str() {
        "select" | "Selector" => json!({
            "type": "selector",
            "tag": group.name,
            "outbounds": refs,
            "default": refs.first()
        }),
        "url-test" | "URLTest" => json!({
            "type": "urltest",
            "tag": group.name,
            "outbounds": refs,
            "url": test_url,
            "interval": interval,
            "tolerance": 50
        }),
        "fallback" | "Fallback" => {
            // sing-box 无 fallback 类型，降级为 urltest（高容忍度近似故障转移）
            warnings.push(format!(
                "策略组 '{}' 类型为 Fallback，sing-box 无对应类型，已降级为 urltest",
                group.name
            ));
            json!({
                "type": "urltest",
                "tag": group.name,
                "outbounds": refs,
                "url": test_url,
                "interval": interval,
                "tolerance": 0
            })
        }
        // sing-box 的 loadbalance 出站 1.12 才引入，当前内核 1.11 不支持，
        // 降级为 urltest（自动选延迟最低节点，行为最接近）
        "load-balance" | "LoadBalance" => {
            warnings.push(format!(
                "策略组 '{}' 为 load-balance，sing-box 1.11 不支持，已降级为 url-test",
                group.name
            ));
            json!({
                "type": "urltest",
                "tag": group.name,
                "outbounds": refs,
                "url": test_url,
                "interval": interval,
                "tolerance": 50
            })
        }
        other => {
            warnings.push(format!(
                "策略组 '{}' 类型 {} 未知，降级为 selector",
                group.name, other
            ));
            json!({
                "type": "selector",
                "tag": group.name,
                "outbounds": refs
            })
        }
    }
}

// ---------------- 小工具 ----------------

/// Clash 策略引用 → sing-box 内置 tag
fn map_ref(reference: &str) -> String {
    match reference {
        "DIRECT" => "direct".into(),
        "REJECT" | "REJECT-DROP" => "block".into(),
        other => other.into(),
    }
}

/// 将 src 的字段并入 dst（已存在的键会被覆盖）
fn merge(dst: &mut Value, src: &Value) {
    if let (Some(dst_obj), Some(src_obj)) = (dst.as_object_mut(), src.as_object()) {
        for (k, v) in src_obj {
            dst_obj.insert(k.clone(), v.clone());
        }
    }
}

/// 递归移除 null 字段
fn strip_nulls(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let null_keys: Vec<String> = map
                .iter()
                .filter(|(_, v)| v.is_null())
                .map(|(k, _)| k.clone())
                .collect();
            for key in null_keys {
                map.remove(&key);
            }
            for v in map.values_mut() {
                strip_nulls(v);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                strip_nulls(v);
            }
        }
        _ => {}
    }
}
