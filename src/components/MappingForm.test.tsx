import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App as AntApp } from "antd";
import "../i18n";

const mocks = vi.hoisted(() => ({
  accountList: vi.fn(),
  gistListForAccount: vi.fn(),
  mappingCreate: vi.fn(),
  mappingUpdate: vi.fn(),
  dialogOpen: vi.fn(),
}));

vi.mock("../api/ipc", async () => {
  const actual = await vi.importActual<typeof import("../api/ipc")>("../api/ipc");
  // 保留 IpcError 等真实导出，仅替换 IPC 函数为 mock
  return { ...actual, ...mocks };
});
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.dialogOpen }));

import MappingForm, { gistOptionLabel } from "./MappingForm";

describe("gistOptionLabel", () => {
  const base = {
    id: "abcdefgh1234",
    description: null,
    title: null,
    updatedAt: "2026-10-15T08:00:00Z",
    files: [] as string[],
  };

  it("标题优先于描述", () => {
    expect(
      gistOptionLabel({ ...base, title: "配置笔记", description: "描述" }),
    ).toBe("配置笔记 · 2026-10-15");
  });

  it("超长中文描述按字符截断为 30 字", () => {
    const long = "笔".repeat(31);
    expect(gistOptionLabel({ ...base, description: long })).toBe(
      `${"笔".repeat(30)}… · 2026-10-15`,
    );
  });

  it("含首文件名与多文件计数", () => {
    expect(
      gistOptionLabel({ ...base, title: "t", files: ["a.md", "b.md", "c.md"] }),
    ).toBe("t · a.md +2 · 2026-10-15");
  });

  it("全无时回退短 id", () => {
    expect(gistOptionLabel(base)).toBe("#abcdefgh · 2026-10-15");
  });
});

const account = {
  id: "acc1",
  name: "个人 GitHub",
  provider: "github" as const,
  baseUrl: "https://api.github.com",
  tokenRef: "r",
};

beforeEach(() => {
  vi.clearAllMocks();
  mocks.accountList.mockResolvedValue([account]);
  mocks.gistListForAccount.mockResolvedValue([]);
});

const renderOpenForm = () =>
  render(
    <AntApp>
      <MappingForm open editing={null} onCancel={() => {}} onSaved={() => {}} />
    </AntApp>,
  );

/** 手动 gist 模式填完必填项并返回确定按钮 */
const fillRequired = async () => {
  await userEvent.type(await screen.findByLabelText(/本地文件/), "C:/notes/todo.md");
  const accountSelect = await screen.findByLabelText(/账号/);
  await userEvent.click(accountSelect);
  await userEvent.click(
    await screen.findByText(/个人 GitHub/, { selector: ".ant-select-item-option-content" }),
  );
  await userEvent.click(await screen.findByRole("radio", { name: /手动输入/ }));
  await userEvent.type(await screen.findByRole("textbox", { name: /^Gist$/ }), "abc123");
};

describe("MappingForm 名称字段", () => {
  it("填 Gist 内文件名时，空白的名称自动跟随", async () => {
    renderOpenForm();
    await userEvent.type(await screen.findByLabelText(/Gist 内文件名/), "todo.md");
    const nameInput = (await screen.findByLabelText(/^名称$/)) as HTMLInputElement;
    await waitFor(() => expect(nameInput.value).toBe("todo.md"));
  });

  it("名称已手输时不被文件名覆盖", async () => {
    renderOpenForm();
    await userEvent.type(await screen.findByLabelText(/^名称$/), "自定义名");
    await userEvent.type(await screen.findByLabelText(/Gist 内文件名/), "todo.md");
    const nameInput = (await screen.findByLabelText(/^名称$/)) as HTMLInputElement;
    expect(nameInput.value).toBe("自定义名");
  });

  it("提交载荷包含 name", async () => {
    mocks.mappingCreate.mockResolvedValue({} as never);
    renderOpenForm();
    await fillRequired();
    await userEvent.type(await screen.findByLabelText(/Gist 内文件名/), "todo.md");
    await userEvent.clear(await screen.findByLabelText(/^名称$/));
    await userEvent.type(await screen.findByLabelText(/^名称$/), "我的映射");
    await userEvent.click(await screen.findByRole("button", { name: /^OK$|^确\s*定$/ }));
    await waitFor(() => expect(mocks.mappingCreate).toHaveBeenCalledTimes(1));
    expect(mocks.mappingCreate).toHaveBeenCalledWith(
      expect.objectContaining({ name: "我的映射" }),
    );
  });
});
