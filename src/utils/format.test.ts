import { describe, expect, it } from "vitest";

import { formatBytes, formatSpeed, formatUptime } from "./format";

describe("formatBytes", () => {
  it("0 或负数 → 0 B", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(-100)).toBe("0 B");
  });

  it("B 级别", () => {
    expect(formatBytes(512)).toBe("512.0 B");
  });

  it("KB 级别", () => {
    expect(formatBytes(2 * 1024)).toBe("2.0 KB");
  });

  it("MB / GB / TB 逐级进位", () => {
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.0 MB");
    expect(formatBytes(3 * 1024 ** 3)).toBe("3.0 GB");
    expect(formatBytes(10 * 1024 ** 4)).toBe("10.0 TB");
  });

  it("超过 TB 仍停在 TB", () => {
    expect(formatBytes(1024 ** 5)).toBe("1024.0 TB");
  });
});

describe("formatSpeed", () => {
  it("追加 /s 后缀", () => {
    expect(formatSpeed(2 * 1024)).toBe("2.0 KB/s");
  });
});

describe("formatUptime", () => {
  it("仅秒", () => {
    expect(formatUptime(45)).toBe("45s");
  });

  it("分+秒补零", () => {
    expect(formatUptime(125)).toBe("2m 05s");
  });

  it("时+分+秒补零", () => {
    expect(formatUptime(3661)).toBe("1h 01m 01s");
  });
});
