import type { TFunction } from "i18next";
import { IpcError } from "../api/ipc";

/**
 * IPC 错误的展示文案：结构化错误按 code 走 i18n（errors.<code>）。
 * 底层错误类文案含 {{detail}} 插值（原始错误信息）；ACCOUNT_IN_USE 的 count 参与插值；
 * 未知 code/非结构化错误回退 detail 原文。
 */
export function ipcErrorMessage(e: unknown, t: TFunction): string {
  if (e instanceof IpcError && e.code) {
    return t(`errors.${e.code}`, {
      defaultValue: e.detail,
      detail: e.detail,
      ...(e.count !== undefined ? { count: e.count } : {}),
    });
  }
  return String(e);
}

/**
 * 引擎状态消息（状态列 Tooltip）的展示文案：后端存 `{"code":"...","detail":"..."}`
 * JSON 串，按 code 走 i18n（statusMsg.<code>，detail 进插值）；非 JSON 原样返回。
 */
export function statusMessageText(msg: string | null, t: TFunction): string | undefined {
  if (!msg) return undefined;
  const jsonPart = msg.match(/\{[\s\S]*\}/)?.[0];
  if (jsonPart) {
    try {
      const parsed = JSON.parse(jsonPart) as { code?: unknown; detail?: unknown };
      if (parsed && typeof parsed.code === "string") {
        const detail = typeof parsed.detail === "string" ? parsed.detail : msg;
        return t(`statusMsg.${parsed.code}`, { defaultValue: detail, detail });
      }
    } catch {
      // 非 JSON：按原文
    }
  }
  return msg;
}
