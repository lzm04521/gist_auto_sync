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
