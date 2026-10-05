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

use std::collections::{BTreeMap, HashMap, HashSet};
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
    /// route.final 指向的出站（主策略组链入口，用于解析主选择组）
    pub final_group: String,
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
    /// 策略组通过 `use:` 引用的节点集合（多数为远程 http 订阅片段）
    #[serde(default, rename = "proxy-providers")]
    proxy_providers: HashMap<String, ProviderDef>,
}

/// 供外部模块（订阅更新流程）读取 proxy-providers 的公开视图
#[derive(Deserialize, Default)]
pub struct ClashProfileView {
    #[serde(default, rename = "proxy-providers")]
    pub proxy_providers: HashMap<String, ProviderView>,
}

#[derive(Deserialize)]
pub struct ProviderView {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub url: Option<String>,
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
    /// Clash vless/vmess 常用 servername 字段（与 sni 同义）
    #[serde(default, rename = "servername")]
    servername: Option<String>,
    /// uTLS 指纹（如 chrome）；CDN 类节点缺失时易被 reset
    #[serde(default, rename = "client-fingerprint")]
    client_fingerprint: Option<String>,
    /// Reality 配置（vless + tls 节点；缺失 public-key 时视为普通 TLS）
    #[serde(default, rename = "reality-opts")]
    reality_opts: Option<RealityOpts>,
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
    /// hysteria2 混淆类型（Clash 中通常为 salamander）
    #[serde(default)]
    obfs: Option<String>,
    /// hysteria2 混淆密码（字段 obfs-password）
    #[serde(default, rename = "obfs-password")]
    obfs_password: Option<String>,
    #[serde(default, rename = "http-opts")]
    http_opts: Option<HttpOpts>,
    #[serde(default, rename = "h2-opts")]
    h2_opts: Option<H2Opts>,
}

#[derive(Deserialize)]
struct RealityOpts {
    #[serde(default, rename = "public-key")]
    public_key: Option<String>,
    #[serde(default, rename = "short-id")]
    short_id: Option<String>,
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
struct HttpOpts {
    /// Clash path 为列表（HTTP/2 多路复用风格），取首个
    #[serde(default)]
    path: Option<Vec<String>>,
    #[serde(default)]
    method: Option<String>,
    /// Clash 头值为列表，sing-box 同样期望字符串列表
    #[serde(default)]
    headers: Option<BTreeMap<String, Vec<String>>>,
}

#[derive(Deserialize)]
struct H2Opts {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    host: Option<Vec<String>>,
}

/// proxy-provider 定义（仅 http 可由客户端拉取；file 依赖运行时目录，不支持）
#[derive(Deserialize)]
struct ProviderDef {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    url: Option<String>,
}

/// provider 载荷（标准格式只含 proxies 列表；也可能是完整 Clash 配置）
#[derive(Deserialize, Default)]
struct ProviderPayload {
    #[serde(default)]
    proxies: Vec<ClashNode>,
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
    /// 通过 proxy-provider 引入的节点集合（provider 名称）
    #[serde(default, rename = "use")]
    use_providers: Vec<String>,
}

// ---------------- 入口 ----------------

pub fn build(
    profiles: &[(String, String, String)],
    custom_rules: &[String],
    mode: RunMode,
    settings: &UserSettings,
    data_dir: &Path,
    work_dir: &Path,
    api_addr: &str,
    api_secret: &str,
) -> Result<BuiltConfig> {
    // 单条 sing-box JSON：原样透传，但自定义规则仍可前置合并（见 merge_json_custom）
    if profiles.len() == 1 && profiles[0].2.trim().starts_with('{') {
        let trimmed = profiles[0].2.trim();
        let mut config: Value = serde_json::from_str(trimmed)
            .context("profile looks like JSON but failed to parse")?;
        let mut warnings = vec!["sing-box JSON 配置原样透传，未做模式/端口合并".into()];
        merge_json_custom(&mut config, custom_rules, &mut warnings);
        let final_group = config
            .get("route")
            .and_then(|r| r.get("final"))
            .and_then(|f| f.as_str())
            .unwrap_or_default()
            .to_string();
        return Ok(BuiltConfig {
            config,
            warnings,
            final_group,
        });
    }

    let mut warnings = Vec::new();
    let mut known_tags: HashSet<String> =
        ["direct", "block", "dns-out"].map(str::to_string).into_iter().collect();

    let mut outbounds = vec![
        json!({ "type": "direct", "tag": "direct" }),
        json!({ "type": "block", "tag": "block" }),
        json!({ "type": "dns", "tag": "dns-out" }),
    ];

    let mut route_rules: Vec<Value> = Vec::new();
    let mut final_tag: Option<String> = None;
    let mut first_group_tag: Option<String> = None;
    let mut total_nodes = 0usize;
    let mut total_groups = 0usize;

    // ---- 预解析：profile + provider 节点，并构建冲突改名表 ----
    // 改名必须在处理节点/组之前算完，自定义规则才能解析到跨订阅的改名标签
    struct ParsedProfile {
        name: String,
        profile: ClashProfile,
        rename: HashMap<String, String>,
        /// provider 名称 → 节点（文件缺失的 provider 不出现）
        provider_nodes: HashMap<String, Vec<ClashNode>>,
    }

    let mut parsed_list: Vec<ParsedProfile> = Vec::new();
    // 合并全部订阅的改名表：原名 → 改名（重名冲突时先订阅优先）
    let mut global_rename: HashMap<String, String> = HashMap::new();

    for (url, sub_name, text) in profiles {
        let profile: ClashProfile = serde_yaml::from_str(text.trim())
            .with_context(|| format!("无法解析订阅「{sub_name}」为 Clash YAML（请检查订阅格式）"))?;

        // 读取已落盘的 proxy-provider 节点（update_subscription 时按 URL 下载）
        let mut provider_nodes: HashMap<String, Vec<ClashNode>> = HashMap::new();
        for (pname, pdef) in &profile.proxy_providers {
            if pdef.kind != "http" {
                warnings.push(format!(
                    "proxy-provider '{pname}' 类型为 {}（仅支持 http），已忽略",
                    pdef.kind
                ));
                continue;
            }
            let path = crate::core::store::provider_path(data_dir, url, pname);
            match std::fs::read_to_string(&path) {
                Ok(payload) => {
                    let parsed: ProviderPayload = serde_yaml::from_str(&payload).unwrap_or_default();
                    provider_nodes.insert(pname.clone(), parsed.proxies);
                }
                Err(_) => warnings.push(format!(
                    "proxy-provider '{pname}' 节点文件缺失（请更新订阅），已忽略"
                )),
            }
        }

        // 与已注册标签冲突的本订阅节点/provider节点/组名 → "[订阅名] 原名"
        let mut rename: HashMap<String, String> = HashMap::new();
        let candidates = profile
            .proxies
            .iter()
            .map(|p| &p.name)
            .chain(provider_nodes.values().flatten().map(|p| &p.name))
            .chain(profile.proxy_groups.iter().map(|g| &g.name));
        for name in candidates {
            if known_tags.contains(name) && !rename.contains_key(name) {
                rename.insert(name.clone(), format!("[{sub_name}] {name}"));
            }
        }
        // 把本订阅的全部标签（含改名结果）登记，供后续订阅做冲突判定
        for name in profile
            .proxies
            .iter()
            .map(|p| &p.name)
            .chain(provider_nodes.values().flatten().map(|p| &p.name))
            .chain(profile.proxy_groups.iter().map(|g| &g.name))
        {
            let mapped = rename.get(name).cloned().unwrap_or_else(|| name.clone());
            known_tags.insert(mapped);
        }
        for (from, to) in &rename {
            if !global_rename.contains_key(from) {
                global_rename.insert(from.clone(), to.clone());
            }
        }

        parsed_list.push(ParsedProfile {
            name: sub_name.clone(),
            profile,
            rename,
            provider_nodes,
        });
    }

    // 自定义规则：优先级最高，插在所有订阅规则之前；策略名走全局改名
    let custom_apply = |name: &str| -> String {
        let mapped = map_ref(name);
        global_rename.get(&mapped).cloned().unwrap_or(mapped)
    };
    for line in custom_rules {
        let parsed = parse_rule_line(line, &custom_apply, &mut warnings);
        if final_tag.is_none() {
            final_tag = parsed.final_target;
        }
        if let Some(rule) = parsed.rule {
            route_rules.push(rule);
        }
    }

    // ---- 逐订阅产出出站/组/规则 ----
    for parsed in &parsed_list {
        let ParsedProfile {
            name: sub_name,
            profile,
            rename,
            provider_nodes,
        } = parsed;

        let apply = |name: &str| -> String {
            let mapped = map_ref(name);
            rename.get(&mapped).cloned().unwrap_or(mapped)
        };

        // 订阅自身节点
        for node in &profile.proxies {
            emit_node(&mut outbounds, &mut known_tags, &mut warnings, node, &apply, &mut total_nodes);
        }
        // provider 节点
        for nodes in provider_nodes.values() {
            for node in nodes {
                emit_node(
                    &mut outbounds,
                    &mut known_tags,
                    &mut warnings,
                    node,
                    &apply,
                    &mut total_nodes,
                );
            }
        }

        // 策略组：成员 = proxies + use: 引用的 provider 节点
        for group in &profile.proxy_groups {
            let tag = apply(&group.name);
            if !known_tags.insert(tag.clone()) {
                warnings.push(format!("节点/组标签重复，已跳过：{}", tag));
                continue;
            }

            let mut members: Vec<String> = group.proxies.clone();
            for pname in &group.use_providers {
                match provider_nodes.get(pname) {
                    Some(nodes) => members.extend(nodes.iter().map(|n| n.name.clone())),
                    None => warnings.push(format!(
                        "策略组 '{}' 引用的 proxy-provider '{pname}' 不可用，已忽略",
                        group.name
                    )),
                }
            }
            if members.is_empty() {
                warnings.push(format!(
                    "策略组 '{}' 没有任何成员（proxies/use 均为空或不可用）",
                    group.name
                ));
            }

            let mut outbound = group_to_outbound(group, &members, &mut warnings, rename);
            outbound["tag"] = json!(tag.clone());
            if first_group_tag.is_none() {
                first_group_tag = Some(tag);
            }
            outbounds.push(outbound);
            total_groups += 1;
        }

        // 规则 → sing-box route.rules（多订阅按记录顺序拼接，先订阅优先）
        for line in &profile.rules {
            let parsed = parse_rule_line(line, &apply, &mut warnings);
            if final_tag.is_none() {
                final_tag = parsed.final_target;
            }
            if let Some(rule) = parsed.rule {
                route_rules.push(rule);
            }
        }
    }

    // 未显式 MATCH：优先全部订阅中的首个策略组，否则 direct
    let final_tag = final_tag
        .or(first_group_tag)
        .unwrap_or_else(|| "direct".into());

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
    // dns-remote 用 DoH 直连：UDP 53 的 8.8.8.8 在国内会被污染，
    // 非 .cn 后缀的国内域名（如 baidu.com）若被解析到假 IP，direct 出站也会 TLS 握手失败。
    // geosite cn 覆盖国内域名（含 .com），".cn" 后缀规则仅作兜底。
    let dns = json!({
        "servers": [
            { "tag": "dns-remote", "address": "https://1.1.1.1/dns-query", "detour": "direct" },
            { "tag": "dns-local", "address": "223.5.5.5", "detour": "direct" }
        ],
        "rules": [
            { "geosite": ["cn"], "server": "dns-local" },
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

    if total_nodes == 0 && total_groups == 0 {
        bail!("订阅中没有任何节点或策略组");
    }

    Ok(BuiltConfig {
        config,
        warnings,
        final_group: final_tag,
    })
}

// ---------------- 构建小助手 ----------------

/// 转换并登记一个节点出站；apply 决定最终 tag（含冲突改名）
fn emit_node(
    outbounds: &mut Vec<Value>,
    known_tags: &mut HashSet<String>,
    warnings: &mut Vec<String>,
    node: &ClashNode,
    apply: &dyn Fn(&str) -> String,
    total_nodes: &mut usize,
) {
    let tag = apply(&node.name);
    if !known_tags.insert(tag.clone()) {
        warnings.push(format!("节点/组标签重复，已跳过：{}", tag));
        return;
    }
    match node_to_outbound(node) {
        Some((mut outbound, note)) => {
            outbound["tag"] = json!(tag);
            if let Some(note) = note {
                warnings.push(format!("节点 '{}': {}", node.name, note));
            }
            outbounds.push(outbound);
            *total_nodes += 1;
        }
        None => warnings.push(format!(
            "不支持的节点类型，已跳过 '{}' ({})",
            node.name, node.kind
        )),
    }
}

/// sing-box JSON 透传时，把自定义规则前置合并到 route.rules（只改 JSON，不影响透传语义）
fn merge_json_custom(config: &mut Value, custom_rules: &[String], warnings: &mut Vec<String>) {
    if custom_rules.is_empty() {
        return;
    }
    let mut extra: Vec<Value> = Vec::new();
    for line in custom_rules {
        let parsed = parse_rule_line(line, &|n| map_ref(n), warnings);
        if parsed.final_target.is_some() {
            warnings.push(format!("JSON 透传模式忽略自定义 MATCH 行：{}", line));
        }
        if let Some(rule) = parsed.rule {
            extra.push(rule);
        }
    }
    if extra.is_empty() {
        return;
    }
    // route 缺失时安全创建（避免可变索引插入 null 污染）
    if config.get("route").is_none() {
        config["route"] = json!({});
    }
    let route = config.get_mut("route").expect("route ensured");
    let existing = route
        .get("rules")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    let added = extra.len();
    let merged: Vec<Value> = extra.into_iter().chain(existing).collect();
    route["rules"] = json!(merged);
    warnings.push(format!(
        "已在 JSON 配置前置合并 {} 条自定义规则",
        added
    ));
}

// ---------------- 规则行解析 ----------------

/// 一条 Clash 规则行的解析结果
struct ParsedLine {
    /// route.rules 项（MATCH/FINAL/空行/被跳过时为 None）
    rule: Option<Value>,
    /// MATCH/FINAL 的目标出站（仅本行是 MATCH/FINAL 时）
    final_target: Option<String>,
}

/// 解析一条 Clash 规则行；apply 负责策略名映射（内置 tag 转换 + 多订阅冲突改名）
fn parse_rule_line(
    line: &str,
    apply: &dyn Fn(&str) -> String,
    warnings: &mut Vec<String>,
) -> ParsedLine {
    const NONE: ParsedLine = ParsedLine {
        rule: None,
        final_target: None,
    };
    let parts: Vec<&str> = line.split(',').map(str::trim).collect();
    if parts.is_empty() || parts[0].is_empty() {
        return NONE;
    }

    let kind = parts[0].to_ascii_uppercase();

    // MATCH / FINAL 不产生规则项，只影响 route.final
    if kind == "MATCH" || kind == "FINAL" {
        return ParsedLine {
            rule: None,
            final_target: Some(apply(parts.get(1).copied().unwrap_or("DIRECT"))),
        };
    }

    let Some(payload) = parts.get(1).copied() else {
        warnings.push(format!("规则缺少参数，已跳过：{}", line));
        return NONE;
    };
    let policy = parts.get(2).copied().unwrap_or("DIRECT");
    let outbound = apply(policy);

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

    ParsedLine {
        rule,
        final_target: None,
    }
}

// ---------------- 节点转换 ----------------

/// 节点转换；返回 (出站, 可选警告)，未知类型返回 None
fn node_to_outbound(node: &ClashNode) -> Option<(Value, Option<String>)> {
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
            (o, None)
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
            let note = attach_transport(&mut o, node);
            (o, note)
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
            let note = attach_transport(&mut o, node);
            (o, note)
        }

        "trojan" => {
            let mut o = json!({
                "type": "trojan",
                "password": node.password.clone().unwrap_or_default(),
            });
            merge(&mut o, &base);
            // Trojan 始终 TLS
            o["tls"] = tls_block(node);
            let note = attach_transport(&mut o, node);
            (o, note)
        }

        "tuic" => {
            let mut o = json!({
                "type": "tuic",
                "uuid": node.uuid.clone().unwrap_or_default(),
                "password": node.password.clone().unwrap_or_default(),
                "congestion_control": node.congestion_control.clone().unwrap_or_else(|| "bbr".into()),
            });
            merge(&mut o, &base);
            o["tls"] = tls_block(node);
            (o, None)
        }

        "hysteria2" | "hy2" => {
            let mut o = json!({
                "type": "hysteria2",
                "password": node.password.clone().unwrap_or_default(),
            });
            merge(&mut o, &base);
            o["tls"] = tls_block(node);
            // salamander 混淆：Clash obfs / obfs-password → sing-box obfs 块
            if let Some(kind) = &node.obfs
                && !kind.is_empty()
            {
                let mut block = json!({ "type": kind });
                if let Some(password) = &node.obfs_password
                    && !password.is_empty()
                {
                    block["password"] = json!(password);
                }
                o["obfs"] = block;
            }
            (o, None)
        }

        "hysteria" => {
            let mut o = json!({
                "type": "hysteria",
                "auth_str": node.password.clone().unwrap_or_default(),
            });
            merge(&mut o, &base);
            o["tls"] = tls_block(node);
            (o, None)
        }

        _ => return None,
    };

    // 移除值为 null 的可选字段，保持输出干净
    let (mut value, note) = outbound;
    strip_nulls(&mut value);
    Some((value, note))
}

/// 给出站附加传输层；返回可选警告（如 h2 不支持）
fn attach_transport(outbound: &mut Value, node: &ClashNode) -> Option<String> {
    let (transport, warning) = build_transport(node);
    if let Some(transport) = transport {
        outbound["transport"] = transport;
    }
    warning
}

fn build_tls(node: &ClashNode) -> Option<Value> {
    if !node.tls.unwrap_or(false) {
        return None;
    }
    Some(tls_block(node))
}

/// 节点 SNI 解析顺序：sni → servername → ws Host 头 → server。
/// 直接用 server（常为 CDN IP）作 SNI 会被对端拒绝（tls: handshake failure）
fn node_sni(node: &ClashNode) -> String {
    for cand in [&node.sni, &node.servername] {
        if let Some(s) = cand
            && !s.is_empty()
        {
            return s.clone();
        }
    }
    if let Some(host) = node
        .ws_opts
        .as_ref()
        .and_then(|w| w.headers.as_ref())
        .and_then(|h| h.get("Host").or_else(|| h.get("host")))
        && !host.is_empty()
    {
        return host.clone();
    }
    node.server.clone()
}

/// 完整 TLS 块：SNI / insecure / alpn / uTLS 指纹（trojan/hy2 等强制 TLS 的节点也用）
fn tls_block(node: &ClashNode) -> Value {
    let mut tls = json!({
        "enabled": true,
        "server_name": node_sni(node),
        "insecure": node.skip_cert_verify.unwrap_or(false),
    });
    if let Some(alpn) = &node.alpn
        && !alpn.is_empty()
    {
        tls["alpn"] = json!(alpn);
    }
    if let Some(fp) = &node.client_fingerprint
        && !fp.is_empty()
    {
        tls["utls"] = json!({ "enabled": true, "fingerprint": fp });
    }
    // Reality：sing-box 字段为 public_key / short_id；缺 public-key 不启用
    if let Some(reality) = &node.reality_opts
        && let Some(pk) = &reality.public_key
        && !pk.is_empty()
    {
        let mut r = json!({ "enabled": true, "public_key": pk });
        if let Some(sid) = &reality.short_id
            && !sid.is_empty()
        {
            r["short_id"] = json!(sid);
        }
        tls["reality"] = r;
    }
    tls
}

/// 传输层转换；返回 (传输块, 可选警告)。
/// 支持 ws/grpc/http；h2 sing-box 无对应传输，返回警告且不附加传输。
fn build_transport(node: &ClashNode) -> (Option<Value>, Option<String>) {
    match node.network.as_deref() {
        Some("ws") => {
            let mut transport = json!({
                "type": "ws",
                "path": node.ws_opts.as_ref().and_then(|w| w.path.clone()).unwrap_or_else(|| "/".into())
            });
            if let Some(headers) = node.ws_opts.as_ref().and_then(|w| w.headers.clone()) {
                transport["headers"] = json!(headers);
            }
            (Some(transport), None)
        }
        Some("grpc") => (Some(json!({
            "type": "grpc",
            "service_name": node.grpc_opts.as_ref().and_then(|g| g.service_name.clone()).unwrap_or_default()
        })), None),
        Some("http") => {
            let mut transport = json!({ "type": "http" });
            if let Some(opts) = &node.http_opts {
                if let Some(paths) = &opts.path
                    && let Some(first) = paths.first()
                {
                    transport["path"] = json!(first);
                }
                if let Some(method) = &opts.method
                    && !method.is_empty()
                {
                    transport["method"] = json!(method);
                }
                if let Some(headers) = &opts.headers {
                    // Host 头 → sing-box http transport 的 host 列表
                    if let Some(host) = headers.get("Host").or_else(|| headers.get("host")) {
                        transport["host"] = json!(host);
                    }
                    // 其余头原样保留（值为字符串列表，与 sing-box 契约一致）
                    let rest: BTreeMap<&String, &Vec<String>> = headers
                        .iter()
                        .filter(|(k, _)| k.to_ascii_lowercase() != "host")
                        .collect();
                    if !rest.is_empty() {
                        transport["headers"] = json!(rest);
                    }
                }
            }
            (Some(transport), None)
        }
        Some("h2") => (
            None,
            Some(
                "节点使用 HTTP/2 (h2) 传输，sing-box 无对应传输类型，已移除传输（节点可能不可达）"
                    .into(),
            ),
        ),
        _ => (None, None),
    }
}

// ---------------- 策略组转换 ----------------

fn group_to_outbound(
    group: &ClashGroup,
    members: &[String],
    warnings: &mut Vec<String>,
    rename: &HashMap<String, String>,
) -> Value {
    // 成员引用同步应用冲突重命名（多订阅合并时指向本订阅的改名节点/组）
    let refs: Vec<Value> = members
        .iter()
        .map(|r| {
            let mapped = map_ref(r);
            let mapped = rename.get(&mapped).cloned().unwrap_or(mapped);
            json!(mapped)
        })
        .collect();
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
