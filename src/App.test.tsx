import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import "./i18n";
import App from "./App";

describe("App scaffold", () => {
  it("渲染布局壳：应用名与主导航菜单", () => {
    render(<App />);
    expect(screen.getByText("Gist Auto Sync")).toBeInTheDocument();
    // i18n 已加载：菜单为中文（路由页标题也含同名文案，故用 getAll）
    expect(screen.getAllByText("仪表盘").length).toBeGreaterThan(0);
    expect(screen.getAllByText("账号").length).toBeGreaterThan(0);
  });
});
