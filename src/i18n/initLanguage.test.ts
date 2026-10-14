import { describe, it, expect, vi } from "vitest";

const mocks = vi.hoisted(() => ({ settingsGet: vi.fn() }));
vi.mock("../api/ipc", async () => {
  const actual = await vi.importActual<typeof import("../api/ipc")>("../api/ipc");
  return { ...actual, settingsGet: mocks.settingsGet };
});

import i18n, { initLanguage } from "./index";

describe("initLanguage", () => {
  it("按 settings_get 的语言设置 i18n 语言", async () => {
    mocks.settingsGet.mockResolvedValue({
      debounceSeconds: 3,
      pollIntervalMinutes: 5,
      autoStart: true,
      logLevel: "info",
      language: "en",
    });
    await initLanguage();
    expect(i18n.language).toBe("en");
    expect(i18n.t("menu.dashboard")).toBe("Dashboard");
    // 恢复默认，避免影响其他用例
    await i18n.changeLanguage("zh-CN");
  });

  it("读取失败时保持默认语言", async () => {
    mocks.settingsGet.mockRejectedValue(new Error("not in tauri"));
    await initLanguage();
    expect(i18n.language).toBe("zh-CN");
  });
});
