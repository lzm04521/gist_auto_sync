import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App as AntApp } from "antd";
import i18n from "../i18n";

const mocks = vi.hoisted(() => ({
  settingsGet: vi.fn(),
  settingsUpdate: vi.fn(),
  appOpenLogDir: vi.fn(),
}));

vi.mock("../api/ipc", async () => {
  const actual = await vi.importActual<typeof import("../api/ipc")>("../api/ipc");
  // 保留 IpcError 等真实导出，仅替换 IPC 函数为 mock
  return { ...actual, ...mocks };
});

import Settings from "./Settings";

const defaults = {
  debounceSeconds: 3,
  pollIntervalMinutes: 5,
  autoStart: true,
  logLevel: "info",
  language: "zh-CN",
  autoCheckUpdate: true,
};

beforeEach(() => {
  vi.clearAllMocks();
  mocks.settingsGet.mockResolvedValue({ ...defaults });
  i18n.changeLanguage("zh-CN");
});

describe("Settings page", () => {
  it("settings_get 回填表单", async () => {
    render(
      <AntApp>
        <Settings />
      </AntApp>,
    );
    const debounceInput = await screen.findByLabelText(/防抖秒数/);
    await waitFor(() => expect((debounceInput as HTMLInputElement).value).toBe("3"));
    expect(screen.getByLabelText(/轮询间隔/)).toBeInTheDocument();
  });

  it("修改后提交 settingsUpdate 载荷正确", async () => {
    mocks.settingsUpdate.mockResolvedValue(undefined);
    render(
      <AntApp>
        <Settings />
      </AntApp>,
    );
    const debounceInput = await screen.findByLabelText(/防抖秒数/);
    await userEvent.clear(debounceInput);
    await userEvent.type(debounceInput, "10");
    await userEvent.click(screen.getByRole("button", { name: /保 存|保存|Save/ }));

    await waitFor(() => expect(mocks.settingsUpdate).toHaveBeenCalledTimes(1));
    expect(mocks.settingsUpdate).toHaveBeenCalledWith({ ...defaults, debounceSeconds: 10 });
  });

  it("切换语言立即生效", async () => {
    render(
      <AntApp>
        <Settings />
      </AntApp>,
    );
    expect(await screen.findByLabelText(/防抖秒数/)).toBeInTheDocument();
    const langSelect = screen.getByLabelText(/界面语言/);
    await userEvent.click(langSelect);
    await userEvent.click(await screen.findByText("English", { selector: ".ant-select-item-option-content" }));
    // 保存前即切换显示语言
    await waitFor(() => expect(screen.getByLabelText(/Debounce/)).toBeInTheDocument());
  });

  it("数值越界前端拦截，不调用后端", async () => {
    render(
      <AntApp>
        <Settings />
      </AntApp>,
    );
    const debounceInput = await screen.findByLabelText(/防抖秒数/);
    await userEvent.clear(debounceInput);
    await userEvent.type(debounceInput, "99999");
    await userEvent.click(screen.getByRole("button", { name: /保 存|保存|Save/ }));
    expect(await screen.findByText(/防抖秒数须在/)).toBeInTheDocument();
    expect(mocks.settingsUpdate).not.toHaveBeenCalled();
  });

  it("打开日志目录按钮调用 app_open_log_dir", async () => {
    mocks.appOpenLogDir.mockResolvedValue(undefined);
    render(
      <AntApp>
        <Settings />
      </AntApp>,
    );
    await userEvent.click(await screen.findByRole("button", { name: /打开日志目录/ }));
    await waitFor(() => expect(mocks.appOpenLogDir).toHaveBeenCalledTimes(1));
  });
});
