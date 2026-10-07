import type { OutboundMode, RunMode } from "@/stores/core";

/**
 * 接入模式：流量「如何进入」Prism。
 * 与出站模式是两个独立维度——命名上刻意避免与出站模式重名
 * （如不再使用"全局""仅规则"等容易与出站模式混淆的措辞）。
 */
export const RUN_MODES: { key: RunMode; title: string; desc: string }[] = [
  {
    key: "systemProxy",
    title: "系统代理",
    desc: "自动修改系统代理设置，接管支持代理的应用流量（推荐）",
  },
  {
    key: "tun",
    title: "TUN 虚拟网卡",
    desc: "虚拟网卡接管整机全部流量，需管理员/root 权限",
  },
  {
    key: "ruleOnly",
    title: "手动代理",
    desc: "不修改系统设置，需在应用内手动填写代理端口后按规则分流",
  },
];

/**
 * 出站模式：流量「最终走向」。
 * 标题均带"分流/代理/直连"后缀，避免与接入模式的短词混淆。
 */
export const OUTBOUND_MODES: {
  key: OutboundMode;
  title: string;
  desc: string;
}[] = [
  {
    key: "rule",
    title: "规则分流",
    desc: "按订阅与自定义规则决定直连或走节点（默认）",
  },
  {
    key: "global",
    title: "全局代理",
    desc: "全部流量走 GLOBAL 组当前所选节点",
  },
  {
    key: "direct",
    title: "全部直连",
    desc: "全部流量直连，不经过任何节点",
  },
];

/** 接入模式与出站模式的展示标题集合，用于校验是否存在重名混淆 */
export const RUN_MODE_TITLES = RUN_MODES.map((m) => m.title);
export const OUTBOUND_MODE_TITLES = OUTBOUND_MODES.map((m) => m.title);
