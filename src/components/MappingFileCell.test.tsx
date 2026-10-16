import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import "../i18n";
import MappingFileCell, { mappingDisplayName } from "./MappingFileCell";
import type { Mapping } from "../types";

const mapping = (over: Partial<Mapping>): Mapping => ({
  id: "m1",
  name: "",
  localPath: "C:/notes/todo.md",
  accountId: "acc1",
  gistId: "abcdefgh12345678",
  gistFilename: "todo.md",
  uploadEnabled: true,
  downloadEnabled: true,
  enabled: true,
  hooks: {
    preUpload: { mode: "file", scriptPath: "", scriptContent: "", inlineKind: "powershell", onlyWhenChanged: true, blockOnFailure: true },
    postUpload: { mode: "file", scriptPath: "", scriptContent: "", inlineKind: "powershell", onlyWhenChanged: true, blockOnFailure: true },
    preDownload: { mode: "file", scriptPath: "", scriptContent: "", inlineKind: "powershell", onlyWhenChanged: true, blockOnFailure: true },
    postDownload: { mode: "file", scriptPath: "", scriptContent: "", inlineKind: "powershell", onlyWhenChanged: true, blockOnFailure: true },
  },
  ...over,
});

describe("mappingDisplayName", () => {
  it("回退链：name → gistFilename → 本地文件名", () => {
    expect(mappingDisplayName(mapping({ name: "工作笔记" }))).toBe("工作笔记");
    // name 空 → 回退 gist 文件名
    expect(mappingDisplayName(mapping({}))).toBe("todo.md");
    // gist 文件名也空 → 回退本地文件名
    expect(mappingDisplayName(mapping({ gistFilename: "" }))).toBe("todo.md");
  });
});

describe("MappingFileCell", () => {
  it("三行渲染：名称加粗 + 本地完整路径块 + gist 截断 id 块", () => {
    render(<MappingFileCell mapping={mapping({ name: "工作笔记" })} />);
    expect(screen.getByText("工作笔记").tagName).toBe("STRONG");
    expect(screen.getByText("C:/notes/todo.md")).toBeInTheDocument();
    expect(screen.getByText("abcdefgh/todo.md")).toBeInTheDocument();
  });

  it("两个信息块均为单行省略样式", () => {
    const { container } = render(<MappingFileCell mapping={mapping({})} />);
    const ellipsized = container.querySelectorAll<HTMLElement>('[style*="text-overflow"]');
    expect(ellipsized.length).toBeGreaterThanOrEqual(2);
    for (const el of ellipsized) {
      expect(el.style.textOverflow).toBe("ellipsis");
      expect(el.style.whiteSpace).toBe("nowrap");
    }
  });
});
