import { describe, expect, it } from "vitest";

import { delayClass, delayText, type Delay } from "./latency";

describe("delayText", () => {
  it("undefined → 未测速", () => {
    expect(delayText(undefined)).toBe("未测速");
  });

  it("0 → 超时", () => {
    expect(delayText(0)).toBe("超时");
  });

  it("正数 → N ms", () => {
    expect(delayText(120)).toBe("120 ms");
    expect(delayText(0 as Delay)).toBe("超时");
  });
});

describe("delayClass", () => {
  it("undefined（未测速）→ 灰色", () => {
    expect(delayClass(undefined)).toBe("text-gray-500");
  });

  it("0（超时）→ 灰色", () => {
    expect(delayClass(0)).toBe("text-gray-500");
  });

  it("<100 → 绿色", () => {
    expect(delayClass(1)).toBe("text-latency-good");
    expect(delayClass(99)).toBe("text-latency-good");
  });

  it("100~299 → 黄色", () => {
    expect(delayClass(100)).toBe("text-latency-medium");
    expect(delayClass(299)).toBe("text-latency-medium");
  });

  it(">=300 → 红色（橙红）", () => {
    expect(delayClass(300)).toBe("text-latency-bad");
    expect(delayClass(5000)).toBe("text-latency-bad");
  });
});
