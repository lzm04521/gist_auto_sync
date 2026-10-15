import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App as AntApp } from "antd";
import "../i18n";

const mocks = vi.hoisted(() => ({
  mappingList: vi.fn(),
  syncIsPaused: vi.fn(),
  syncSetPaused: vi.fn(),
  syncManual: vi.fn(),
  mappingSetEnabled: vi.fn(),
  conflictPreview: vi.fn(),
  statusCallback: [] as Array<(e: unknown) => void>,
}));

vi.mock("../api/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/ipc")>();
  return {
    ...actual,
    mappingList: mocks.mappingList,
    syncIsPaused: mocks.syncIsPaused,
    syncSetPaused: mocks.syncSetPaused,
    syncManual: mocks.syncManual,
    mappingSetEnabled: mocks.mappingSetEnabled,
    conflictPreview: mocks.conflictPreview,
    onStatusEvent: (cb: (e: unknown) => void) => {
      mocks.statusCallback.push(cb);
      return Promise.resolve(() => {});
    },
  };
});

import Dashboard from "./Dashboard";

function view(overrides: Partial<MappingViewShape> = {}): MappingViewShape {
  return {
    mapping: {
      id: "m1",
      name: "",
      localPath: "C:/notes/todo.md",
      accountId: "acc1",
      gistId: "abc123",
      gistFilename: "todo.md",
      uploadEnabled: true,
      downloadEnabled: true,
      enabled: true,
    },
    status: "idle",
    message: null,
    lastSyncedAt: null,
    ...overrides,
  };
}

interface MappingViewShape {
  mapping: {
    id: string;
    name: string;
    localPath: string;
    accountId: string;
    gistId: string;
    gistFilename: string;
    uploadEnabled: boolean;
    downloadEnabled: boolean;
    enabled: boolean;
  };
  status: string;
  message: string | null;
  lastSyncedAt: string | null;
}

beforeEach(() => {
  vi.clearAllMocks();
  mocks.statusCallback.length = 0;
  mocks.syncIsPaused.mockResolvedValue(false);
});

describe("Dashboard", () => {
  it("冲突行显示红色状态与解决按钮", async () => {
    mocks.mappingList.mockResolvedValue([view({ status: "conflict" })]);
    render(
      <AntApp>
        <Dashboard />
      </AntApp>,
    );
    expect(await screen.findByText("C:/notes/todo.md")).toBeInTheDocument();
    expect(screen.getByText("冲突").closest(".ant-tag")).toHaveClass("ant-tag-red");
    expect(screen.getByRole("button", { name: /解决冲突/ })).toBeInTheDocument();
  });

  it("解决弹窗渲染双栏 diff 并按方向调用 syncManual", async () => {
    mocks.mappingList.mockResolvedValue([view({ status: "conflict" })]);
    mocks.conflictPreview.mockResolvedValue({
      local: "line1\nlocal changed\nline3",
      remote: "line1\nremote changed\nline3",
    });
    mocks.syncManual.mockResolvedValue("idle");
    render(
      <AntApp>
        <Dashboard />
      </AntApp>,
    );
    await userEvent.click(await screen.findByRole("button", { name: /解决冲突/ }));

    // diff 双栏：左含 local changed（removed 高亮），右含 remote changed（added 高亮）
    await waitFor(() =>
      expect(screen.getByText("local changed")).toBeInTheDocument(),
    );
    expect(screen.getByText("remote changed")).toBeInTheDocument();
    expect(screen.getByText("local changed").dataset.diff).toBe("removed");
    expect(screen.getByText("remote changed").dataset.diff).toBe("added");

    await userEvent.click(screen.getByRole("button", { name: /用远端覆盖本地/ }));
    await waitFor(() => expect(mocks.syncManual).toHaveBeenCalledWith("m1", "download"));
    // 解决后弹窗关闭
    await waitFor(() =>
      expect(screen.queryByText(/用远端覆盖本地/)).not.toBeInTheDocument(),
    );
  });

  it("状态事件推送实时更新行内状态", async () => {
    mocks.mappingList.mockResolvedValue([view({ status: "idle" })]);
    render(
      <AntApp>
        <Dashboard />
      </AntApp>,
    );
    expect(await screen.findByText("正常")).toBeInTheDocument();
    mocks.statusCallback[0]({
      mappingId: "m1",
      status: "error",
      message: "网络错误",
      lastSyncedAt: null,
    });
    await waitFor(() => expect(screen.getByText("错误")).toBeInTheDocument());
  });

  it("全局暂停开关调用 sync_set_paused", async () => {
    mocks.mappingList.mockResolvedValue([]);
    render(
      <AntApp>
        <Dashboard />
      </AntApp>,
    );
    const sw = await screen.findByRole("switch");
    await userEvent.click(sw);
    await waitFor(() => expect(mocks.syncSetPaused).toHaveBeenCalledWith(true));
  });

  it("立即上传按钮按方向调用 syncManual", async () => {
    mocks.mappingList.mockResolvedValue([view()]);
    mocks.syncManual.mockResolvedValue("idle");
    render(
      <AntApp>
        <Dashboard />
      </AntApp>,
    );
    await userEvent.click(await screen.findByRole("button", { name: /立即上传/ }));
    await waitFor(() => expect(mocks.syncManual).toHaveBeenCalledWith("m1", "upload"));
  });
});

describe("Dashboard missing recovery", () => {
  it("Missing 行显示恢复按钮并可打开处理弹窗", async () => {
    mocks.mappingList.mockResolvedValue([view({ status: "missing" })]);
    mocks.conflictPreview.mockResolvedValue({ local: null, remote: "远端还在的内容" });
    render(
      <AntApp>
        <Dashboard />
      </AntApp>,
    );
    await userEvent.click(await screen.findByRole("button", { name: /恢\s*复/ }));
    // 弹窗打开并渲染远端内容（本地缺失一侧为空）
    await waitFor(() => expect(screen.getByText("远端还在的内容")).toBeInTheDocument());
  });
});
