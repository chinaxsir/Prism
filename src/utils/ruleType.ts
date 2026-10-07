/** 规则类型对应的展示色 class（抽离自 Rules 页，便于单测） */

const RULE_TYPE_COLORS: Record<string, string> = {
  DOMAIN: "text-blue-400",
  "DOMAIN-SUFFIX": "text-green-400",
  "DOMAIN-KEYWORD": "text-yellow-400",
  "IP-CIDR": "text-purple-400",
  GEOIP: "text-pink-400",
  GEOSITE: "text-orange-400",
  "PROCESS-NAME": "text-red-400",
  RULESET: "text-cyan-400",
};

export function ruleTypeClass(type: string): string {
  return RULE_TYPE_COLORS[type] ?? "text-gray-400";
}
