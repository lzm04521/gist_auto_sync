import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import "../i18n";
import DiffView from "./DiffView";

describe("DiffView 行级 diff（V0.1 已实现，本测试锁行为）", () => {
  it("删除行红底、新增行绿底（data-diff 标记）", () => {
    // JSX 字符串属性不处理 \n 转义，须用 JS 表达式传真实换行
    render(<DiffView local={"a\nb"} remote={"a\nc"} />);
    const removed = screen.getByText("b");
    expect(removed).toHaveAttribute("data-diff", "removed");
    const added = screen.getByText("c");
    expect(added).toHaveAttribute("data-diff", "added");
    // 公共行两侧各一份
    expect(screen.getAllByText("a")).toHaveLength(2);
  });

  it("两栏内容相同时显示空态", () => {
    render(<DiffView local="a" remote="a" />);
    expect(screen.getByText("暂无数据")).toBeInTheDocument();
  });
});
