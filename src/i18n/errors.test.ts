import { describe, it, expect } from "vitest";
import i18n from "./index";
import { ipcErrorMessage } from "./errors";
import { IpcError } from "../api/ipc";

const t = i18n.t.bind(i18n);

describe("ipcErrorMessage", () => {
  it("结构化错误按 code 取 i18n 文案", () => {
    const msg = ipcErrorMessage(
      new IpcError("MAPPING_LOCAL_DUPLICATE", "该本地文件已被其他映射使用"),
      t,
    );
    expect(msg).toBe(i18n.t("errors.MAPPING_LOCAL_DUPLICATE"));
    expect(msg).not.toContain("detail");
  });

  it("带插值的错误合并 detail 与 count", () => {
    const msg = ipcErrorMessage(
      new IpcError("ACCOUNT_IN_USE", "原始信息", 3),
      t,
    );
    expect(msg).toContain("3");
    expect(i18n.t("errors.ACCOUNT_IN_USE", { count: 3 })).toContain("3");
  });

  it("未知 code 回退 detail 原文", () => {
    expect(ipcErrorMessage(new IpcError("SOME_FUTURE_CODE", "后端新错误"), t)).toBe(
      "后端新错误",
    );
  });

  it("非结构化错误显示字符串形式", () => {
    expect(ipcErrorMessage(new Error("boom"), t)).toContain("boom");
    expect(ipcErrorMessage("plain text", t)).toBe("plain text");
  });
});
