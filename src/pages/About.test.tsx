import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App as AntApp } from "antd";
import i18n from "../i18n";

const mocks = vi.hoisted(() => ({
  settingsGet: vi.fn(),
  settingsUpdate: vi.fn(),
  getVersion: vi.fn(),
  check: vi.fn(),
  downloadAndInstall: vi.fn(),
  relaunch: vi.fn(),
  reset: vi.fn(),
}));

vi.mock("../api/ipc", async () => {
  const actual = await vi.importActual<typeof import("../api/ipc")>("../api/ipc");
  return { ...actual, settingsGet: mocks.settingsGet, settingsUpdate: mocks.settingsUpdate };
});

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: mocks.getVersion,
}));

type UpdaterState = {
  status: string;
  version: string | null;
  notes: string | null;
  progress: number | null;
  error: string | null;
  check: ReturnType<typeof vi.fn>;
  downloadAndInstall: ReturnType<typeof vi.fn>;
  relaunch: ReturnType<typeof vi.fn>;
  reset: ReturnType<typeof vi.fn>;
};

const baseState = (): UpdaterState => ({
  status: "idle",
  version: null,
  notes: null,
  progress: null,
  error: null,
  check: mocks.check,
  downloadAndInstall: mocks.downloadAndInstall,
  relaunch: mocks.relaunch,
  reset: mocks.reset,
});

let updaterState = baseState();

vi.mock("../hooks/useUpdater", () => ({
  useUpdater: () => updaterState,
  useStartupAutoCheck: () => {},
}));

import About from "./About";

const fullSettings = (autoCheckUpdate: boolean) => ({
  debounceSeconds: 3,
  pollIntervalMinutes: 5,
  autoStart: true,
  logLevel: "info",
  language: "zh-CN",
  autoCheckUpdate,
});

function renderAbout() {
  return render(
    <AntApp>
      <About />
    </AntApp>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  updaterState = baseState();
  mocks.settingsGet.mockResolvedValue(fullSettings(true));
  mocks.getVersion.mockResolvedValue("0.6.0");
  mocks.downloadAndInstall.mockResolvedValue(true);
  i18n.changeLanguage("zh-CN");
});

describe("About 页", () => {
  it("渲染当前版本、检查更新按钮与自动检查开关", async () => {
    renderAbout();
    expect(await screen.findByText("0.6.0")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /检查更新/ })).toBeInTheDocument();
    const sw = screen.getByRole("switch", { name: /自动检查更新/ });
    expect(sw).toBeInTheDocument();
  });

  it("点检查更新调用 hook，发现新版本弹窗展示版本与纯文本说明", async () => {
    const user = userEvent.setup();
    const view = renderAbout();
    await screen.findByText("0.6.0");
    await user.click(screen.getByRole("button", { name: /检查更新/ }));
    expect(mocks.check).toHaveBeenCalledTimes(1);

    // 同一实例内推进 hook 状态：available + 含 HTML 的说明
    updaterState = { ...updaterState, status: "available", version: "0.4.1", notes: "<img src=x onerror=alert(1)>" };
    view.rerender(
      <AntApp>
        <About />
      </AntApp>,
    );
    expect(await screen.findByText(/0\.4\.1/)).toBeInTheDocument();
    // 说明必须以文本节点呈现（不作为 HTML 渲染，防注入）
    expect(screen.getByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
    expect(document.querySelector("img[src='x']")).toBeNull();
  });

  it("弹窗内立即更新走下载安装，完成后可一键重启", async () => {
    const user = userEvent.setup();
    updaterState = { ...updaterState, status: "available", version: "0.4.1", notes: "修复" };
    const view = renderAbout();
    const updateBtn = await screen.findByRole("button", { name: /立即更新/ });
    await user.click(updateBtn);
    expect(mocks.downloadAndInstall).toHaveBeenCalledTimes(1);

    updaterState = { ...updaterState, status: "downloaded", progress: 100 };
    view.rerender(
      <AntApp>
        <About />
      </AntApp>,
    );
    const relaunchBtn = await screen.findByRole("button", { name: /立即重启/ });
    await user.click(relaunchBtn);
    expect(mocks.relaunch).toHaveBeenCalledTimes(1);
  });

  it("已是最新版本时提示成功", async () => {
    updaterState = { ...updaterState, status: "upToDate" };
    renderAbout();
    expect(await screen.findByText("已是最新版本")).toBeInTheDocument();
  });

  it("检查失败时提示错误", async () => {
    updaterState = { ...updaterState, status: "failed", error: "network down" };
    renderAbout();
    expect(await screen.findByText(/检查更新失败/)).toBeInTheDocument();
    expect(screen.getByText(/network down/)).toBeInTheDocument();
  });

  it("切换自动检查开关即时保存，载荷含 autoCheckUpdate", async () => {
    const user = userEvent.setup();
    renderAbout();
    const sw = await screen.findByRole("switch", { name: /自动检查更新/ });
    await user.click(sw);
    await waitFor(() =>
      expect(mocks.settingsUpdate).toHaveBeenCalledWith(
        expect.objectContaining({ autoCheckUpdate: false }),
      ),
    );
    expect(await screen.findByText("设置已保存")).toBeInTheDocument();
  });

  it("下载中点稍后不清状态，完成自动重开弹窗可一键重启", async () => {
    const user = userEvent.setup();
    updaterState = { ...updaterState, status: "available", version: "0.4.1", notes: "修复" };
    const view = renderAbout();
    await screen.findByRole("button", { name: /立即更新/ });

    // 下载中关闭弹窗：状态不得被重置
    updaterState = { ...updaterState, status: "downloading", progress: 50 };
    view.rerender(
      <AntApp>
        <About />
      </AntApp>,
    );
    await user.click(screen.getByRole("button", { name: /稍后|稍 后/ }));
    expect(mocks.reset).not.toHaveBeenCalled();

    // 下载完成：弹窗自动重开，重启入口不丢失
    updaterState = { ...updaterState, status: "downloaded", progress: 100 };
    view.rerender(
      <AntApp>
        <About />
      </AntApp>,
    );
    const relaunchBtn = await screen.findByRole("button", { name: /立即重启/ });
    await user.click(relaunchBtn);
    expect(mocks.relaunch).toHaveBeenCalledTimes(1);
  });

  it("已下载待重启时再点检查更新重开弹窗，不重复检查与下载", async () => {
    const user = userEvent.setup();
    updaterState = { ...updaterState, status: "downloaded", version: "0.4.1", progress: 100 };
    const view = renderAbout();
    // downloaded 首次进入即弹出重启提示，先关闭
    await screen.findByRole("button", { name: /立即重启/ });
    await user.click(screen.getByRole("button", { name: /稍后|稍 后/ }));

    await user.click(screen.getByRole("button", { name: /检查更新/ }));
    expect(mocks.check).not.toHaveBeenCalled();
    expect(mocks.downloadAndInstall).not.toHaveBeenCalled();
    expect(await screen.findByRole("button", { name: /立即重启/ })).toBeInTheDocument();
    view.unmount();
  });

  it("英文文案跟随语言切换", async () => {
    i18n.changeLanguage("en");
    renderAbout();
    expect(await screen.findByRole("button", { name: /Check for updates/ })).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: /automatically/i })).toBeInTheDocument();
  });
});

describe("App 菜单集成", () => {
  it("关于入口位于设置之后，点击可导航", async () => {
    const user = userEvent.setup();
    // 占位兄弟页面，避免渲染整站 IPC 依赖
    vi.mock("./Dashboard", () => ({ default: () => <div>dashboard-stub</div> }));
    vi.mock("./Mappings", () => ({ default: () => <div>mappings-stub</div> }));
    vi.mock("./History", () => ({ default: () => <div>history-stub</div> }));
    vi.mock("./Accounts", () => ({ default: () => <div>accounts-stub</div> }));
    const { default: App } = await import("../App");
    render(<App />);

    const items = await screen.findAllByRole("menuitem");
    const labels = items.map((el) => el.textContent);
    expect(labels).toEqual(["仪表盘", "同步映射", "历史", "日志", "账号", "设置", "关于"]);

    await user.click(screen.getByText("关于"));
    expect(await screen.findByText("0.6.0")).toBeInTheDocument();
  });
});
