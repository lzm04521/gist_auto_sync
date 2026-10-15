import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App as AntApp } from "antd";
import "../i18n";

const mocks = vi.hoisted(() => ({
  syncLogList: vi.fn(),
}));

vi.mock("../api/ipc", async () => {
  const actual = await vi.importActual<typeof import("../api/ipc")>("../api/ipc");
  return { ...actual, ...mocks };
});

import Logs from "./Logs";
import type { SyncLogEntry } from "../types";

const entries: SyncLogEntry[] = [
  {
    ts: "2026-10-15T08:00:00Z",
    mappingId: "m1",
    label: "工作笔记",
    trigger: "localSettled",
    direction: "upload",
    outcome: { kind: "changed", added: 3, removed: 1 },
  },
  {
    ts: "2026-10-15T07:00:00Z",
    mappingId: "m1",
    label: "工作笔记",
    trigger: "manualUpload",
    direction: null,
    outcome: { kind: "noChange" },
  },
  {
    ts: "2026-10-15T06:00:00Z",
    mappingId: "m2",
    label: "配置",
    trigger: "poll",
    direction: "download",
    outcome: { kind: "error", code: "provider.get_failed" },
  },
];

beforeEach(() => {
  vi.clearAllMocks();
  mocks.syncLogList.mockResolvedValue(entries);
});

const renderPage = () =>
  render(
    <AntApp>
      <Logs />
    </AntApp>,
  );

describe("Logs page", () => {
  it("渲染各列：触发/方向/结果，增删行数分色", async () => {
    renderPage();
    expect(await screen.findByText("自动 · 文件变更")).toBeInTheDocument();
    expect(screen.getByText("手动")).toBeInTheDocument();
    expect(screen.getByText(/上传/)).toBeInTheDocument();
    expect(screen.getByText("+3")).toBeInTheDocument();
    expect(screen.getByText("-1")).toBeInTheDocument();
    expect(screen.getByText("无变化")).toBeInTheDocument();
  });

  it("失败行以原生 title 暴露错误码", async () => {
    renderPage();
    expect(await screen.findByTitle("provider.get_failed")).toBeInTheDocument();
  });

  it("触发列筛选浮层可打开并勾选筛选项", async () => {
    renderPage();
    const filterButtons = screen
      .getAllByRole("button")
      .filter((b) => b.className.includes("ant-table-filter-trigger"));
    expect(filterButtons.length).toBeGreaterThan(0);
    await userEvent.click(filterButtons[0]);
    const dd = await waitFor(() => {
      const el = document.querySelector(".ant-table-filter-dropdown");
      expect(el).toBeTruthy();
      return el as HTMLElement;
    });
    // 三个筛选项（行级过滤由 antd 内建 onFilter 承载，此处验证浮层与勾选交互）
    expect(within(dd).getByText("自动 · 文件变更")).toBeInTheDocument();
    expect(within(dd).getByText("自动 · 轮询")).toBeInTheDocument();
    const boxes = within(dd).getAllByRole("checkbox");
    await userEvent.click(boxes[boxes.length - 1]);
    await waitFor(() =>
      expect((within(dd).getAllByRole("checkbox")[boxes.length - 1] as HTMLInputElement).checked).toBe(true),
    );
  });
});
