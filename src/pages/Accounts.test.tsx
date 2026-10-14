import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { App as AntApp } from "antd";
import userEvent from "@testing-library/user-event";
import "../i18n";

const mocks = vi.hoisted(() => ({
  accountList: vi.fn(),
  accountCreate: vi.fn(),
  accountUpdate: vi.fn(),
  accountDelete: vi.fn(),
  accountTest: vi.fn(),
}));

vi.mock("../api/ipc", async () => {
  const actual = await vi.importActual<typeof import("../api/ipc")>("../api/ipc");
  // 保留 IpcError 等真实导出，仅替换 IPC 函数为 mock
  return { ...actual, ...mocks };
});

import Accounts from "./Accounts";
import { IpcError } from "../api/ipc";

const githubAccount = {
  id: "a1",
  name: "个人 GitHub",
  provider: "github" as const,
  baseUrl: "https://api.github.com",
  tokenRef: "gistsync:account:a1",
};
const opengistAccount = {
  id: "a2",
  name: "自建实例",
  provider: "opengist" as const,
  baseUrl: "https://gist.example.com",
  tokenRef: "gistsync:account:a2",
};

beforeEach(() => {
  vi.clearAllMocks();
});

describe("Accounts page", () => {
  it("渲染账号列表并区分类型徽标", async () => {
    mocks.accountList.mockResolvedValue([githubAccount, opengistAccount]);
    render(
      <AntApp>
        <Accounts />
      </AntApp>,
    );
    expect(await screen.findByText("个人 GitHub")).toBeInTheDocument();
    expect(screen.getByText("GitHub Gist")).toBeInTheDocument();
    expect(screen.getByText("Opengist")).toBeInTheDocument();
    expect(screen.getByText("https://gist.example.com")).toBeInTheDocument();
  });

  it("新增 Opengist 时显示实例地址输入，GitHub 时隐藏", async () => {
    mocks.accountList.mockResolvedValue([]);
    render(
      <AntApp>
        <Accounts />
      </AntApp>,
    );
    await userEvent.click(await screen.findByText("新建账号"));

    const providerSelect = await screen.findByLabelText(/类型/);
    await userEvent.click(providerSelect);
    // 选 Opengist → 出现实例地址
    await userEvent.click(await screen.findByText("Opengist", { selector: ".ant-select-item-option-content" }));
    expect(await screen.findByLabelText(/实例地址/)).toBeInTheDocument();
  });

  it("提交创建时载荷为 camelCase 且含 token", async () => {
    mocks.accountList.mockResolvedValue([]);
    mocks.accountCreate.mockResolvedValue({ ...githubAccount });
    render(
      <AntApp>
        <Accounts />
      </AntApp>,
    );
    await userEvent.click(await screen.findByText("新建账号"));

    await userEvent.type(await screen.findByLabelText(/名称/), "测试");
    const providerSelect = await screen.findByLabelText(/类型/);
    await userEvent.click(providerSelect);
    await userEvent.click(await screen.findByText("GitHub Gist", { selector: ".ant-select-item-option-content" }));
    await userEvent.type(await screen.findByLabelText(/访问令牌/), "ghp_test123");

    await userEvent.click(screen.getByRole("button", { name: /确 定|OK|确定/ }));
    await waitFor(() => expect(mocks.accountCreate).toHaveBeenCalledTimes(1));
    expect(mocks.accountCreate).toHaveBeenCalledWith({
      name: "测试",
      provider: "github",
      baseUrl: "",
      token: "ghp_test123",
    });
  });

  it("测试连接成功显示登录名", async () => {
    mocks.accountList.mockResolvedValue([githubAccount]);
    mocks.accountTest.mockResolvedValue({ login: "octocat", name: null });
    render(
      <AntApp>
        <Accounts />
      </AntApp>,
    );
    const buttons = await screen.findAllByText("测试连接");
    await userEvent.click(buttons[0]);
    expect(await screen.findByText(/octocat/)).toBeInTheDocument();
  });

  it("删除被引用账号时按结构化错误码弹出强制删除确认", async () => {
    mocks.accountList.mockResolvedValue([githubAccount]);
    // 后端结构化错误经 ipc.ts 解析后为 IpcError（code=ACCOUNT_IN_USE, count=2）
    mocks.accountDelete.mockRejectedValue(
      new IpcError("ACCOUNT_IN_USE", "该账号仍有同步映射", 2),
    );
    render(
      <AntApp>
        <Accounts />
      </AntApp>,
    );
    // antd 对两个中文字符按钮自动插空格（"删 除"），用 role 名匹配
    await userEvent.click(await screen.findByRole("button", { name: /删\s*除/ }));
    // Popconfirm 确认
    await userEvent.click(await screen.findByRole("button", { name: /^OK$|^确\s*定$/ }));
    // 强制删除确认弹窗展示结构化数量信息
    expect(
      await screen.findByText(/该账号还有 2 条同步映射/),
    ).toBeInTheDocument();
  });
});
