import { afterEach, describe, expect, it, vi } from "vitest";

import { formatRelativeTime } from "./time";

function mockNow(iso: string) {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(iso));
}

afterEach(() => {
  vi.useRealTimers();
});

describe("formatRelativeTime", () => {
  it("0 → 从未更新", () => {
    expect(formatRelativeTime(0)).toBe("从未更新");
  });

  it("刚刚（< 1 分钟）", () => {
    mockNow("2026-10-07T10:00:30Z");
    expect(formatRelativeTime(Date.now() / 1000 - 30)).toBe("刚刚");
  });

  it("分钟前", () => {
    mockNow("2026-10-07T10:00:00Z");
    expect(formatRelativeTime(Date.now() / 1000 - 5 * 60)).toBe("5 分钟前");
  });

  it("小时前", () => {
    mockNow("2026-10-07T10:00:00Z");
    expect(formatRelativeTime(Date.now() / 1000 - 3 * 3600)).toBe("3 小时前");
  });

  it("天前", () => {
    mockNow("2026-10-07T10:00:00Z");
    expect(formatRelativeTime(Date.now() / 1000 - 5 * 86400)).toBe("5 天前");
  });

  it("毫秒时间戳也能识别", () => {
    mockNow("2026-10-07T10:00:00Z");
    // 毫秒级时间戳（>= 1e12）
    expect(formatRelativeTime(Date.now() - 2 * 60 * 1000)).toBe("2 分钟前");
  });

  it("未来时间回退为本地时间字符串", () => {
    mockNow("2026-10-07T10:00:00Z");
    const future = Date.now() + 86400 * 1000;
    const result = formatRelativeTime(future);
    expect(result).not.toMatch(/前$/);
    // 应为 toLocaleString 格式，包含日期与时间
    expect(result.length).toBeGreaterThan(0);
  });

  it("超过 30 天回退为日期字符串", () => {
    mockNow("2026-10-07T10:00:00Z");
    const result = formatRelativeTime(Date.now() - 45 * 86400 * 1000);
    expect(result).not.toMatch(/天前$/);
  });
});
