import { describe, it, expect, vi, beforeEach } from "vitest";

const invokeMock = vi.fn();
const listenMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: (event: string, cb: (ev: { payload: unknown }) => void) =>
    listenMock(event, cb),
}));

import {
  accountCreate,
  appOpenLogDir,
  gistListForAccount,
  onStatusEvent,
  settingsUpdate,
  syncManual,
} from "./ipc";

beforeEach(() => {
  invokeMock.mockReset();
  listenMock.mockReset();
});

describe("ipc bindings", () => {
  it("accountCreate 以 camelCase 载荷调用 account_create", async () => {
    invokeMock.mockResolvedValue({});
    await accountCreate({
      name: "n",
      provider: "opengist",
      baseUrl: "https://g.example.com",
      token: "og_x",
    });
    expect(invokeMock).toHaveBeenCalledWith("account_create", {
      input: {
        name: "n",
        provider: "opengist",
        baseUrl: "https://g.example.com",
        token: "og_x",
      },
    });
  });

  it("syncManual 传递 mappingId/direction", async () => {
    invokeMock.mockResolvedValue("idle");
    await syncManual("m1", "upload");
    expect(invokeMock).toHaveBeenCalledWith("sync_manual", {
      mappingId: "m1",
      direction: "upload",
    });
  });

  it("settingsUpdate 传 settings 对象", async () => {
    invokeMock.mockResolvedValue(undefined);
    const settings = {
      debounceSeconds: 3,
      pollIntervalMinutes: 5,
      autoStart: true,
      logLevel: "info",
      language: "zh-CN",
    };
    await settingsUpdate(settings);
    expect(invokeMock).toHaveBeenCalledWith("settings_update", { settings });
  });

  it("gistListForAccount 传 accountId", async () => {
    invokeMock.mockResolvedValue([]);
    await gistListForAccount("acc-1");
    expect(invokeMock).toHaveBeenCalledWith("gist_list_for_account", {
      accountId: "acc-1",
    });
  });

  it("appOpenLogDir 无参数调用", async () => {
    invokeMock.mockResolvedValue(undefined);
    await appOpenLogDir();
    expect(invokeMock).toHaveBeenCalledWith("app_open_log_dir", undefined);
  });

  it("onStatusEvent 订阅 sync://status 并转发 payload", async () => {
    listenMock.mockResolvedValue(() => {});
    const cb = vi.fn();
    await onStatusEvent(cb);
    expect(listenMock).toHaveBeenCalledTimes(1);
    const [event, handler] = listenMock.mock.calls[0];
    expect(event).toBe("sync://status");
    handler({ payload: { mappingId: "m1", status: "conflict", message: null, lastSyncedAt: null } });
    expect(cb).toHaveBeenCalledWith({
      mappingId: "m1",
      status: "conflict",
      message: null,
      lastSyncedAt: null,
    });
  });
});
