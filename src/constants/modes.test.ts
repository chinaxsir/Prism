import { describe, expect, it } from "vitest";

import {
  OUTBOUND_MODE_TITLES,
  OUTBOUND_MODES,
  RUN_MODE_TITLES,
  RUN_MODES,
} from "./modes";

describe("接入模式 / 出站模式标签（防混淆）", () => {
  it("接入模式包含全部两种 key", () => {
    const keys = RUN_MODES.map((m) => m.key);
    expect(keys).toEqual(expect.arrayContaining(["systemProxy", "tun"]));
    expect(keys).toHaveLength(2);
  });

  it("出站模式包含全部三种 key", () => {
    const keys = OUTBOUND_MODES.map((m) => m.key);
    expect(keys).toEqual(expect.arrayContaining(["rule", "global", "direct"]));
    expect(keys).toHaveLength(3);
  });

  it("每组内部标题不重复", () => {
    expect(new Set(RUN_MODE_TITLES).size).toBe(RUN_MODE_TITLES.length);
    expect(new Set(OUTBOUND_MODE_TITLES).size).toBe(OUTBOUND_MODE_TITLES.length);
  });

  it("接入模式与出站模式之间无重名标题（核心防混淆要求）", () => {
    // 历史问题："TUN 全局" vs 出站"全局"、"仅规则" vs 出站"规则" 造成混淆。
    // 改名后两组标题集合必须完全不相交。
    const runSet = new Set(RUN_MODE_TITLES);
    for (const title of OUTBOUND_MODE_TITLES) {
      expect(runSet.has(title)).toBe(false);
    }
  });

  it("接入模式标题不再使用易混淆的短词（全局/规则）", () => {
    // "全局"会被理解为出站全局代理，"仅规则"会被理解为出站规则分流
    for (const title of RUN_MODE_TITLES) {
      expect(title).not.toBe("全局");
      expect(title).not.toBe("仅规则");
      expect(title).not.toBe("规则");
    }
  });

  it("出站模式标题带分流/代理/直连后缀，语义自洽", () => {
    expect(OUTBOUND_MODE_TITLES).toContain("规则分流");
    expect(OUTBOUND_MODE_TITLES).toContain("全局代理");
    expect(OUTBOUND_MODE_TITLES).toContain("全部直连");
  });

  it("每个模式都有非空描述", () => {
    for (const m of RUN_MODES) {
      expect(m.desc.length).toBeGreaterThan(0);
    }
    for (const m of OUTBOUND_MODES) {
      expect(m.desc.length).toBeGreaterThan(0);
    }
  });
});
