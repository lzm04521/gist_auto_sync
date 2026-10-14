import { describe, it, expect } from "vitest";
import en from "./en.json";
import zhCN from "./zh-CN.json";

function flatten(obj: Record<string, unknown>, prefix = ""): string[] {
  return Object.entries(obj).flatMap(([key, value]) => {
    const path = prefix ? `${prefix}.${key}` : key;
    if (value && typeof value === "object") {
      return flatten(value as Record<string, unknown>, path);
    }
    return [path];
  });
}

describe("i18n resources", () => {
  it("zh-CN 与 en 键集合一致", () => {
    const zh = flatten(zhCN).sort();
    const enKeys = flatten(en).sort();
    expect(zh).toEqual(enKeys);
  });

  it("键集合非空", () => {
    expect(flatten(zhCN).length).toBeGreaterThan(10);
  });
});
