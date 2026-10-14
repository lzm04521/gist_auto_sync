import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App as AntApp } from "antd";
import { MemoryRouter } from "react-router-dom";
import "../i18n";

const mocks = vi.hoisted(() => ({
  mappingList: vi.fn(),
  historyList: vi.fn(),
  historyPreview: vi.fn(),
  gistCommitList: vi.fn(),
  gistCommitPreview: vi.fn(),
  historyRollbackLocal: vi.fn(),
  historyRollbackRemote: vi.fn(),
}));

vi.mock("../api/ipc", async () => {
  const actual = await vi.importActual<typeof import("../api/ipc")>("../api/ipc");
  return { ...actual, ...mocks };
});

import History from "./History";
import { IpcError } from "../api/ipc";

const mappingView = {
  mapping: {
    id: "m1",
    localPath: "C:/notes/todo.md",
    accountId: "acc1",
    gistId: "abc123",
    gistFilename: "todo.md",
    uploadEnabled: true,
    downloadEnabled: true,
    enabled: true,
  },
  status: "idle" as const,
  message: null,
  lastSyncedAt: null,
};

const entry = {
  ts: "2026-10-14T12:30:45.123Z",
  side: "local" as const,
  reason: "download" as const,
  file: "20261014123045123-local.md",
  sizeBytes: 123,
};

const commit = {
  revision: "abcdef1234567890",
  committedAt: "2026-10-13T08:00:00Z",
};

function renderPage(initialEntry = "/history") {
  return render(
    <MemoryRouter initialEntries={[initialEntry]}>
      <AntApp>
        <History />
      </AntApp>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  mocks.mappingList.mockResolvedValue([mappingView]);
  mocks.historyList.mockResolvedValue([entry]);
  mocks.gistCommitList.mockResolvedValue([commit]);
});

describe("History page", () => {
  it("渲染映射下拉与本地归档列表：时间、侧、原因、大小", async () => {
    renderPage();
    expect(await screen.findByText("C:/notes/todo.md")).toBeInTheDocument();
    expect(await screen.findByText("2026-10-14 12:30:45")).toBeInTheDocument();
    expect(screen.getByText("本地")).toBeInTheDocument();
    expect(screen.getByText("下载覆盖")).toBeInTheDocument();
    expect(screen.getByText("123 B")).toBeInTheDocument();
    expect(screen.getByText("20261014123045123-local.md")).toBeInTheDocument();
  });

  it("点对比弹出双栏预览（历史内容 vs 当前本地）", async () => {
    mocks.historyPreview.mockResolvedValue({
      historical: "历史内容A",
      currentLocal: "当前内容B",
    });
    renderPage();
    await screen.findByText("20261014123045123-local.md");
    await userEvent.click(screen.getAllByRole("button", { name: /对 比|对比/ })[0]);
    expect(await screen.findByText("历史内容A")).toBeInTheDocument();
    expect(screen.getByText("当前内容B")).toBeInTheDocument();
    expect(mocks.historyPreview).toHaveBeenCalledWith("m1", "20261014123045123-local.md");
  });

  it("点回滚出现确认弹窗，确认后调用回滚并提示成功", async () => {
    mocks.historyRollbackLocal.mockResolvedValue("idle");
    renderPage();
    await screen.findByText("20261014123045123-local.md");
    await userEvent.click(screen.getAllByRole("button", { name: /回 滚|回滚/ })[0]);
    // 确认弹窗正文
    expect(
      await screen.findByText("将同时覆盖本地与远端文件，并在远端产生一次新提交"),
    ).toBeInTheDocument();
    await userEvent.click(await screen.findByRole("button", { name: /确 定|确定/ }));
    await waitFor(() =>
      expect(mocks.historyRollbackLocal).toHaveBeenCalledWith("m1", "20261014123045123-local.md"),
    );
    expect(await screen.findByText("回滚成功")).toBeInTheDocument();
  });

  it("回滚被拒绝时按错误码展示双语错误", async () => {
    mocks.historyRollbackLocal.mockRejectedValue(
      new IpcError("HISTORY_FILE_NOT_FOUND", "归档文件不存在"),
    );
    renderPage();
    await screen.findByText("20261014123045123-local.md");
    await userEvent.click(screen.getAllByRole("button", { name: /回 滚|回滚/ })[0]);
    await userEvent.click(await screen.findByRole("button", { name: /确 定|确定/ }));
    expect(await screen.findByText("归档文件不存在")).toBeInTheDocument();
  });

  it("远端历史 Tab 渲染短版本号（前 7 位）", async () => {
    renderPage();
    await screen.findByText("20261014123045123-local.md");
    await userEvent.click(await screen.findByRole("tab", { name: /远端历史/ }));
    expect(await screen.findByText("abcdef1")).toBeInTheDocument();
    expect(screen.getByText("2026-10-13 08:00:00")).toBeInTheDocument();
  });

  it("无映射时显示空态", async () => {
    mocks.mappingList.mockResolvedValue([]);
    renderPage();
    expect(await screen.findByText("暂无历史")).toBeInTheDocument();
  });
});
