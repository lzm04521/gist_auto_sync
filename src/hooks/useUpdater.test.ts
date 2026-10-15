import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { act, renderHook } from "@testing-library/react";

const mocks = vi.hoisted(() => ({
  check: vi.fn(),
  relaunch: vi.fn(),
  settingsGet: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-updater", () => ({
  check: mocks.check,
}));

vi.mock("@tauri-apps/plugin-process", () => ({
  relaunch: mocks.relaunch,
}));

vi.mock("../api/ipc", async () => {
  const actual = await vi.importActual<typeof import("../api/ipc")>("../api/ipc");
  return { ...actual, settingsGet: mocks.settingsGet };
});

import { useStartupAutoCheck, useUpdater } from "./useUpdater";

/** 构造插件 Update 形状的对象；downloadAndInstall(onEvent, ...) 依 events 驱动进度回调 */
function fakeUpdate(events: Array<{ event: string; data: Record<string, unknown> }>) {
  return {
    version: "0.4.1",
    currentVersion: "0.4.0",
    body: "修复若干问题",
    date: new Date().toISOString(),
    downloadAndInstall: vi.fn(async (onEvent?: (e: { event: string; data: Record<string, unknown> }) => void) => {
      for (const e of events) onEvent?.(e);
    }),
  };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("useUpdater 状态机", () => {
  it("初始 idle，进度为空", () => {
    const { result } = renderHook(() => useUpdater());
    expect(result.current.status).toBe("idle");
    expect(result.current.progress).toBeNull();
    expect(result.current.version).toBeNull();
    expect(result.current.notes).toBeNull();
    expect(result.current.error).toBeNull();
  });

  it("check 有更新 → available，版本与说明透传", async () => {
    mocks.check.mockResolvedValue(fakeUpdate([]));
    const { result } = renderHook(() => useUpdater());
    await act(() => result.current.check());
    expect(result.current.status).toBe("available");
    expect(result.current.version).toBe("0.4.1");
    expect(result.current.notes).toBe("修复若干问题");
  });

  it("check 无更新 → upToDate", async () => {
    mocks.check.mockResolvedValue(null);
    const { result } = renderHook(() => useUpdater());
    await act(() => result.current.check());
    expect(result.current.status).toBe("upToDate");
  });

  it("check 失败 → failed 且记录错误", async () => {
    mocks.check.mockRejectedValue(new Error("network down"));
    const { result } = renderHook(() => useUpdater());
    await act(() => result.current.check());
    expect(result.current.status).toBe("failed");
    expect(result.current.error).toContain("network down");
  });

  it("downloadAndInstall 事件流推进进度，完成后 downloaded 且进度 100", async () => {
    mocks.check.mockResolvedValue(
      fakeUpdate([
        { event: "Started", data: { contentLength: 1000 } },
        { event: "Progress", data: { chunkLength: 300 } },
        { event: "Progress", data: { chunkLength: 500 } },
        { event: "Finished", data: {} },
      ]),
    );
    const { result } = renderHook(() => useUpdater());
    await act(() => result.current.check());
    let ok = false;
    await act(async () => {
      ok = await result.current.downloadAndInstall();
    });
    expect(ok).toBe(true);
    expect(result.current.status).toBe("downloaded");
    expect(result.current.progress).toBe(100);
  });

  it("downloadAndInstall 失败 → failed，且可再次进入 downloading（可重试）", async () => {
    const bad = fakeUpdate([{ event: "Started", data: { contentLength: 100 } }]);
    bad.downloadAndInstall.mockRejectedValue(new Error("download aborted"));
    mocks.check.mockResolvedValue(bad);
    const { result } = renderHook(() => useUpdater());
    await act(() => result.current.check());
    let ok = false;
    await act(async () => {
      ok = await result.current.downloadAndInstall();
    });
    expect(ok).toBe(false);
    expect(result.current.status).toBe("failed");
    expect(result.current.error).toContain("download aborted");

    // 修复故障后重试：重新检查 → 再下载成功
    const good = fakeUpdate([{ event: "Finished", data: {} }]);
    mocks.check.mockResolvedValue(good);
    await act(() => result.current.check());
    await act(async () => {
      ok = await result.current.downloadAndInstall();
    });
    expect(ok).toBe(true);
    expect(result.current.status).toBe("downloaded");
  });

  it("relaunch 调用插件重启一次", () => {
    mocks.relaunch.mockResolvedValue(undefined);
    const { result } = renderHook(() => useUpdater());
    result.current.relaunch();
    expect(mocks.relaunch).toHaveBeenCalledTimes(1);
  });

  it("reset 回 idle 并清空版本/说明/进度/错误", async () => {
    mocks.check.mockResolvedValue(fakeUpdate([]));
    const { result } = renderHook(() => useUpdater());
    await act(() => result.current.check());
    act(() => result.current.reset());
    expect(result.current.status).toBe("idle");
    expect(result.current.version).toBeNull();
    expect(result.current.notes).toBeNull();
    expect(result.current.progress).toBeNull();
    expect(result.current.error).toBeNull();
  });
});

describe("useStartupAutoCheck 启动自动检查", () => {
  const fullSettings = (autoCheckUpdate: boolean) => ({
    debounceSeconds: 3,
    pollIntervalMinutes: 5,
    autoStart: true,
    logLevel: "info",
    language: "zh-CN",
    autoCheckUpdate,
  });

  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("开关关闭时延迟后完全不发起检查", async () => {
    mocks.settingsGet.mockResolvedValue(fullSettings(false));
    const onAvailable = vi.fn();
    renderHook(() => useStartupAutoCheck(onAvailable));
    await act(() => vi.advanceTimersByTimeAsync(15_000));
    expect(mocks.check).not.toHaveBeenCalled();
    expect(onAvailable).not.toHaveBeenCalled();
  });

  it("开关开启时延迟 10 秒检查恰好一次", async () => {
    mocks.settingsGet.mockResolvedValue(fullSettings(true));
    const onAvailable = vi.fn();
    renderHook(() => useStartupAutoCheck(onAvailable));
    await act(() => vi.advanceTimersByTimeAsync(9_999));
    expect(mocks.check).not.toHaveBeenCalled();
    await act(() => vi.advanceTimersByTimeAsync(1));
    expect(mocks.check).toHaveBeenCalledTimes(1);
  });

  it("检查失败静默：不抛未处理错误、不触发回调", async () => {
    mocks.settingsGet.mockResolvedValue(fullSettings(true));
    mocks.check.mockRejectedValue(new Error("github unreachable"));
    const onAvailable = vi.fn();
    renderHook(() => useStartupAutoCheck(onAvailable));
    await act(() => vi.advanceTimersByTimeAsync(15_000));
    expect(onAvailable).not.toHaveBeenCalled();
  });

  it("发现新版本回调 onAvailable 并带版本号", async () => {
    mocks.settingsGet.mockResolvedValue(fullSettings(true));
    mocks.check.mockResolvedValue({
      version: "0.4.1",
      currentVersion: "0.4.0",
      body: "",
      date: new Date().toISOString(),
      downloadAndInstall: vi.fn(),
    });
    const onAvailable = vi.fn();
    renderHook(() => useStartupAutoCheck(onAvailable));
    await act(() => vi.advanceTimersByTimeAsync(15_000));
    expect(onAvailable).toHaveBeenCalledWith("0.4.1");
  });

  it("卸载后推进时间不再触发检查", async () => {
    mocks.settingsGet.mockResolvedValue(fullSettings(true));
    const onAvailable = vi.fn();
    const { unmount } = renderHook(() => useStartupAutoCheck(onAvailable));
    await act(() => vi.advanceTimersByTimeAsync(1_000));
    unmount();
    await act(() => vi.advanceTimersByTimeAsync(20_000));
    expect(mocks.check).not.toHaveBeenCalled();
    expect(onAvailable).not.toHaveBeenCalled();
  });
});
