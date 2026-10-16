import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App as AntApp } from "antd";
import { MemoryRouter } from "react-router-dom";
import "../i18n";

const mocks = vi.hoisted(() => ({
  mappingList: vi.fn(),
  mappingCreate: vi.fn(),
  mappingUpdate: vi.fn(),
  mappingDelete: vi.fn(),
  accountList: vi.fn(),
  gistListForAccount: vi.fn(),
  dialogOpen: vi.fn(),
}));

vi.mock("../api/ipc", async () => {
  const actual = await vi.importActual<typeof import("../api/ipc")>("../api/ipc");
  // 保留 IpcError 等真实导出，仅替换 IPC 函数为 mock
  return { ...actual, ...mocks };
});
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.dialogOpen }));

import Mappings from "./Mappings";
import { IpcError } from "../api/ipc";
import { emptyEventHook } from "../components/EventHookSection";

const account = {
  id: "acc1",
  name: "个人 GitHub",
  provider: "github" as const,
  baseUrl: "https://api.github.com",
  tokenRef: "r",
};

const mappingView = {
  mapping: {
    id: "m1",
    name: "",
    localPath: "C:/notes/todo.md",
    accountId: "acc1",
    gistId: "abc123",
    gistFilename: "todo.md",
    uploadEnabled: true,
    downloadEnabled: false,
    enabled: true,
  },
  status: "idle" as const,
  message: null,
  lastSyncedAt: null,
};

beforeEach(() => {
  vi.clearAllMocks();
  mocks.accountList.mockResolvedValue([account]);
  mocks.mappingList.mockResolvedValue([mappingView]);
});

const openCreate = async () => {
  await userEvent.click(await screen.findByRole("button", { name: /新建映射/ }));
};

describe("Mappings page", () => {
  it("渲染映射列表：文件路径、gist id、方向开关状态", async () => {
    render(
      <MemoryRouter>
        <AntApp>
          <Mappings />
        </AntApp>
      </MemoryRouter>,
    );
    expect(await screen.findByText("C:/notes/todo.md")).toBeInTheDocument();
    // gist 块只显示 id 前 8 位（此处 fixture 全长 6 位）+ 文件名
    expect(screen.getByText(/abc123\/todo\.md/, { selector: "span" })).toBeInTheDocument();
  });

  it("选择文件后 gistFilename 默认联动为文件名", async () => {
    mocks.dialogOpen.mockResolvedValue("C:/notes/todo.md");
    render(
      <MemoryRouter>
        <AntApp>
          <Mappings />
        </AntApp>
      </MemoryRouter>,
    );
    await openCreate();
    await userEvent.click(await screen.findByRole("button", { name: /选择文件/ }));
    const filenameInput = await screen.findByLabelText(/Gist 内文件名/);
    await waitFor(() => expect((filenameInput as HTMLInputElement).value).toBe("todo.md"));
  });

  it("选择账号后加载 gist 下拉，选项显示描述", async () => {
    mocks.gistListForAccount.mockResolvedValue([
      { id: "abc123", description: "工作笔记", updatedAt: "2026-10-13T08:00:00Z", files: ["todo.md"] },
    ]);
    render(
      <MemoryRouter>
        <AntApp>
          <Mappings />
        </AntApp>
      </MemoryRouter>,
    );
    await openCreate();
    const accountSelect = await screen.findByLabelText(/账号/);
    await userEvent.click(accountSelect);
    await userEvent.click(await screen.findByText(/个人 GitHub/, { selector: ".ant-select-item-option-content" }));

    await waitFor(() => expect(mocks.gistListForAccount).toHaveBeenCalledWith("acc1"));
    const gistSelect = await screen.findByLabelText(/Gist\b|^Gist$/i, { selector: "#gistId, [id=gistId], .ant-select" });
    void gistSelect;
    // 通过下拉验证描述渲染
    await userEvent.click(screen.getByRole("combobox", { name: /gist/i }));
    expect(await screen.findByText(/工作笔记/, { selector: ".ant-select-item-option-content" })).toBeInTheDocument();
  });

  // 表单全链路交互耗时最长（常态 ~10s），高负载下会撞默认 20s 上限，单独放宽
  it("提交载荷 camelCase 且含方向开关", async () => {
    mocks.dialogOpen.mockResolvedValue("C:/notes/todo.md");
    mocks.gistListForAccount.mockResolvedValue([
      { id: "abc123", description: "工作笔记", updatedAt: "2026-10-13T08:00:00Z", files: ["todo.md"] },
    ]);
    mocks.mappingCreate.mockResolvedValue(mappingView.mapping);
    render(
      <MemoryRouter>
        <AntApp>
          <Mappings />
        </AntApp>
      </MemoryRouter>,
    );
    await openCreate();
    await userEvent.click(await screen.findByRole("button", { name: /选择文件/ }));

    const accountSelect = await screen.findByLabelText(/账号/);
    await userEvent.click(accountSelect);
    await userEvent.click(await screen.findByText(/个人 GitHub/, { selector: ".ant-select-item-option-content" }));

    await userEvent.click(screen.getByRole("combobox", { name: /gist/i }));
    await userEvent.click(await screen.findByText(/工作笔记/, { selector: ".ant-select-item-option-content" }));

    // 关掉下载开关（默认开）
    await userEvent.click(screen.getByRole("switch", { name: /远端 → 本地|下载/ }));

    await userEvent.click(screen.getByRole("button", { name: /^OK$|^确\s*定$/ }));
    await waitFor(() => expect(mocks.mappingCreate).toHaveBeenCalledTimes(1));
    expect(mocks.mappingCreate).toHaveBeenCalledWith({
      // 名称留空时自动跟随 Gist 内文件名（V0.5 行为）
      name: "todo.md",
      localPath: "C:/notes/todo.md",
      accountId: "acc1",
      gistId: "abc123",
      gistFilename: "todo.md",
      uploadEnabled: true,
      downloadEnabled: false,
      enabled: true,
      // 事件区未打开 → 提交归一为四个默认（未配置）钩子
      hooks: {
        preUpload: emptyEventHook(),
        postUpload: emptyEventHook(),
        preDownload: emptyEventHook(),
        postDownload: emptyEventHook(),
      },
    });
  }, 40_000);

  it("空路径提交被前端拦截，不调用后端", async () => {
    render(
      <MemoryRouter>
        <AntApp>
          <Mappings />
        </AntApp>
      </MemoryRouter>,
    );
    await openCreate();
    await userEvent.click(await screen.findByRole("button", { name: /^OK$|^确\s*定$/ }));
    // 校验提示出现且未提交
    expect(await screen.findByText(/请选择或输入本地文件路径/)).toBeInTheDocument();
    expect(mocks.mappingCreate).not.toHaveBeenCalled();
  });

  it("后端唯一性错误展示给用户", async () => {
    mocks.dialogOpen.mockResolvedValue("C:/notes/todo.md");
    mocks.gistListForAccount.mockResolvedValue([
      { id: "abc123", description: "工作笔记", updatedAt: "2026-10-13T08:00:00Z", files: ["todo.md"] },
    ]);
    // 结构化错误经 ipc.ts 解析为 IpcError，页面按 code 走 i18n 文案
    mocks.mappingCreate.mockRejectedValue(
      new IpcError("MAPPING_LOCAL_DUPLICATE", "该本地文件已被其他映射使用"),
    );
    render(
      <MemoryRouter>
        <AntApp>
          <Mappings />
        </AntApp>
      </MemoryRouter>,
    );
    await openCreate();
    await userEvent.click(await screen.findByRole("button", { name: /选择文件/ }));
    const accountSelect = await screen.findByLabelText(/账号/);
    await userEvent.click(accountSelect);
    await userEvent.click(await screen.findByText(/个人 GitHub/, { selector: ".ant-select-item-option-content" }));
    await userEvent.click(screen.getByRole("combobox", { name: /gist/i }));
    await userEvent.click(await screen.findByText(/工作笔记/, { selector: ".ant-select-item-option-content" }));
    await userEvent.click(screen.getByRole("button", { name: /^OK$|^确\s*定$/ }));
    expect(await screen.findByText(/该本地文件已被其他映射使用/)).toBeInTheDocument();
  });
});
