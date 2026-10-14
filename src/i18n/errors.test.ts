import { describe, it, expect } from "vitest";
import i18n from "./index";
import { ipcErrorMessage, statusMessageText } from "./errors";
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

describe("statusMessageText", () => {
  it("结构化状态消息按 code 取 i18n 文案", () => {
    const raw = JSON.stringify({
      code: "conflict.dual_change",
      detail: "本地与远端均有未同步修改，请人工选择保留哪边",
    });
    expect(statusMessageText(raw, t)).toBe(i18n.t("statusMsg.conflict.dual_change"));
  });

  it("带插值的消息合并 detail", () => {
    const raw = JSON.stringify({ code: "upload.failed", detail: "网络超时" });
    expect(statusMessageText(raw, t)).toContain("网络超时");
  });

  it("未知 code 与非 JSON 消息回退原文", () => {
    expect(statusMessageText(JSON.stringify({ code: "some.new_code", detail: "新消息" }), t)).toBe(
      "新消息",
    );
    expect(statusMessageText("普通文本消息", t)).toBe("普通文本消息");
  });

  it("空消息返回 undefined（Tooltip 不显示）", () => {
    expect(statusMessageText(null, t)).toBeUndefined();
  });
});
