import { describe, expect, it } from "vitest";

import { ruleTypeClass } from "./ruleType";

describe("ruleTypeClass", () => {
  it("已知类型返回对应颜色", () => {
    expect(ruleTypeClass("DOMAIN")).toBe("text-blue-400");
    expect(ruleTypeClass("DOMAIN-SUFFIX")).toBe("text-green-400");
    expect(ruleTypeClass("GEOIP")).toBe("text-pink-400");
    expect(ruleTypeClass("GEOSITE")).toBe("text-orange-400");
  });

  it("未知类型回退灰色", () => {
    expect(ruleTypeClass("UNKNOWN-TYPE")).toBe("text-gray-400");
    expect(ruleTypeClass("")).toBe("text-gray-400");
  });

  it("所有映射颜色互不相同（便于视觉区分）", () => {
    const types = [
      "DOMAIN",
      "DOMAIN-SUFFIX",
      "DOMAIN-KEYWORD",
      "IP-CIDR",
      "GEOIP",
      "GEOSITE",
      "PROCESS-NAME",
      "RULESET",
    ];
    const colors = types.map(ruleTypeClass);
    expect(new Set(colors).size).toBe(types.length);
  });
});
