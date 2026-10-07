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
  it("undefined → 灰色", () => {
    expect(delayClass(undefined)).toBe("text-gray-500");
  });

  it("0（超时）→ 红色", () => {
    expect(delayClass(0)).toBe("text-latency-bad");
  });

  it("<200 → 绿色", () => {
    expect(delayClass(1)).toBe("text-latency-good");
    expect(delayClass(199)).toBe("text-latency-good");
  });

  it("200~499 → 黄色", () => {
    expect(delayClass(200)).toBe("text-latency-medium");
    expect(delayClass(499)).toBe("text-latency-medium");
  });

  it(">=500 → 红色", () => {
    expect(delayClass(500)).toBe("text-latency-bad");
    expect(delayClass(5000)).toBe("text-latency-bad");
  });
});
