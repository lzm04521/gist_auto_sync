import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App as AntApp, Form } from "antd";
import "../i18n";

const mocks = vi.hoisted(() => ({
  dialogOpen: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.dialogOpen }));

import EventHookSection, { emptyEventHook } from "./EventHookSection";

const initialHook = emptyEventHook();

function renderSection(field: "preUpload" | "postUpload" = "preUpload", isPre = true) {
  return render(
    <AntApp>
      <Form initialValues={{ hooks: { [field]: initialHook } }}>
        <EventHookSection field={field} isPre={isPre} />
      </Form>
    </AntApp>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("EventHookSection", () => {
  it("渲染来源切换、脚本路径与两个开关", () => {
    renderSection();
    expect(screen.getByText("脚本来源")).toBeInTheDocument();
    expect(screen.getByText("脚本文件")).toBeInTheDocument();
    expect(screen.getByText("内联脚本")).toBeInTheDocument();
    expect(screen.getByText("脚本路径")).toBeInTheDocument();
    expect(screen.getByText("仅文件有变动时执行")).toBeInTheDocument();
    expect(screen.getByText("失败时阻止本次同步")).toBeInTheDocument();
  });

  it("阻止开关仅前置事件显示", () => {
    renderSection("postUpload", false);
    expect(screen.queryByText("失败时阻止本次同步")).not.toBeInTheDocument();
    expect(screen.getByText("仅文件有变动时执行")).toBeInTheDocument();
  });

  it("切换来源不丢失另一侧已填内容", async () => {
    renderSection();
    const user = userEvent.setup();
    await user.click(screen.getByText("内联脚本"));
    const textarea = screen.getByRole("textbox");
    await user.type(textarea, "Write-Output ok");
    await user.click(screen.getByText("脚本文件"));
    expect(screen.queryByDisplayValue("Write-Output ok")).not.toBeInTheDocument();
    await user.click(screen.getByText("内联脚本"));
    expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe("Write-Output ok");
  });

  it("浏览按钮带脚本扩展名过滤并回填路径", async () => {
    mocks.dialogOpen.mockResolvedValue("C:/tools/pre.ps1");
    renderSection();
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: /浏\s*览/ }));
    expect(mocks.dialogOpen).toHaveBeenCalledWith(
      expect.objectContaining({
        filters: [{ name: expect.any(String), extensions: ["ps1", "bat", "cmd"] }],
      }),
    );
    expect((screen.getByRole("textbox") as HTMLInputElement).value).toBe("C:/tools/pre.ps1");
  });
});
