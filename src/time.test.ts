import { describe, it, expect } from "vitest";
import { formatTime } from "./time";

/** 期望值独立实现：按本地时区从 Date 各分量拼接（与被测实现同构，防回归为 UTC 截断） */
function localExpected(iso: string): string {
  const d = new Date(iso);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

describe("formatTime", () => {
  it("UTC 输入按本地时区显示，而非 UTC 字符串截断", () => {
    const iso = "2026-10-14T12:30:45.123Z";
    const expected = localExpected(iso);
    expect(formatTime(iso)).toBe(expected);
    // 在非 UTC 时区机器上，UTC 原文截断（旧缺陷行为）必然不等于本地显示
    if (expected !== iso.replace("T", " ").slice(0, 19)) {
      expect(formatTime(iso)).not.toBe(iso.replace("T", " ").slice(0, 19));
    }
  });

  it("无毫秒的 RFC3339 同样正确解析", () => {
    const iso = "2026-10-13T08:00:00Z";
    expect(formatTime(iso)).toBe(localExpected(iso));
  });

  it("null 与空串显示 —", () => {
    expect(formatTime(null)).toBe("—");
    expect(formatTime("")).toBe("—");
  });

  it("非法时间串显示 — 而不是 NaN", () => {
    expect(formatTime("not-a-date")).toBe("—");
  });

  it("输出为 YYYY-MM-DD HH:mm:ss 格式", () => {
    expect(formatTime("2026-01-02T03:04:05Z")).toMatch(
      /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/,
    );
  });
});
