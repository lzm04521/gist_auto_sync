import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  Account,
  AccountInput,
  GistCommit,
  GistSummary,
  GlobalSettings,
  HistoryEntry,
  HistoryPreview,
  Mapping,
  MappingInput,
  MappingStatus,
  MappingView,
  StatusEvent,
  UserInfo,
} from "../types";

// ── 错误结构化 ───────────────────────────────────────────────────────

/**
 * IPC 调用错误。后端命令失败返回 `{"code":"...","detail":"..."}` JSON 串
 * （ACCOUNT_IN_USE 额外带 count）；code 供 i18n 按 errors.<code> 取文案，detail 为兜底。
 */
export class IpcError extends Error {
  readonly code?: string;
  readonly detail: string;
  readonly count?: number;

  constructor(code: string | undefined, detail: string, count?: number) {
    super(detail);
    this.name = "IpcError";
    this.code = code;
    this.detail = detail;
    this.count = count;
  }
}

/** 解析后端错误串：能识别出 code 的包装为 IpcError，否则按原文包装（code=undefined） */
function unwrap<T>(p: Promise<T>): Promise<T> {
  return p.catch((raw: unknown) => {
    const text = typeof raw === "string" ? raw : String(raw);
    const jsonPart = text.match(/\{[\s\S]*\}/)?.[0];
    if (jsonPart) {
      try {
        const parsed = JSON.parse(jsonPart) as {
          code?: unknown;
          detail?: unknown;
          count?: unknown;
        };
        if (parsed && typeof parsed.code === "string") {
          const detail = typeof parsed.detail === "string" ? parsed.detail : text;
          const count = typeof parsed.count === "number" ? parsed.count : undefined;
          return Promise.reject(new IpcError(parsed.code, detail, count));
        }
      } catch {
        // 非法 JSON：按原文展示
      }
    }
    return Promise.reject(new IpcError(undefined, text));
  });
}

// ── 账号 ─────────────────────────────────────────────────────────────

export const accountList = () => unwrap(invoke<Account[]>("account_list"));

export const accountCreate = (input: AccountInput) =>
  unwrap(invoke<Account>("account_create", { input }));

export const accountUpdate = (id: string, input: AccountInput) =>
  unwrap(invoke<void>("account_update", { id, input }));

export const accountDelete = (id: string, force: boolean) =>
  unwrap(invoke<void>("account_delete", { id, force }));

export const accountTest = (id: string) =>
  unwrap(invoke<UserInfo>("account_test", { id }));

// ── 映射 ─────────────────────────────────────────────────────────────

export const mappingList = () => unwrap(invoke<MappingView[]>("mapping_list"));

export const mappingCreate = (input: MappingInput) =>
  unwrap(invoke<Mapping>("mapping_create", { input }));

export const mappingUpdate = (id: string, input: MappingInput) =>
  unwrap(invoke<void>("mapping_update", { id, input }));

export const mappingDelete = (id: string) =>
  unwrap(invoke<void>("mapping_delete", { id }));

export const mappingSetEnabled = (id: string, enabled: boolean) =>
  unwrap(invoke<void>("mapping_set_enabled", { id, enabled }));

// ── gist 选择器 / 同步操作 ───────────────────────────────────────────

export const gistListForAccount = (accountId: string) =>
  unwrap(invoke<GistSummary[]>("gist_list_for_account", { accountId }));

export const syncManual = (mappingId: string, direction: "upload" | "download") =>
  unwrap(invoke<MappingStatus>("sync_manual", { mappingId, direction }));

export const syncSetPaused = (paused: boolean) =>
  unwrap(invoke<void>("sync_set_paused", { paused }));

export const syncIsPaused = () => unwrap(invoke<boolean>("sync_is_paused"));

// ── 设置 ─────────────────────────────────────────────────────────────

export const settingsGet = () => unwrap(invoke<GlobalSettings>("settings_get"));

export const settingsUpdate = (settings: GlobalSettings) =>
  unwrap(invoke<void>("settings_update", { settings }));

export const appOpenLogDir = () => unwrap(invoke<void>("app_open_log_dir"));

/** 冲突弹窗预览：本地 vs 远端当前内容 */
export interface ConflictPreview {
  local: string | null;
  remote: string | null;
}

export const conflictPreview = (mappingId: string) =>
  unwrap(invoke<ConflictPreview>("conflict_preview", { mappingId }));

// ── 历史与版本（V0.3）────────────────────────────────────────────────

export const historyList = (mappingId: string) =>
  unwrap(invoke<HistoryEntry[]>("history_list", { mappingId }));

export const historyPreview = (mappingId: string, fileName: string) =>
  unwrap(invoke<HistoryPreview>("history_preview", { mappingId, fileName }));

export const historyRollbackLocal = (mappingId: string, fileName: string) =>
  unwrap(invoke<MappingStatus>("history_rollback_local", { mappingId, fileName }));

export const gistCommitList = (mappingId: string) =>
  unwrap(invoke<GistCommit[]>("gist_commit_list", { mappingId }));

export const gistCommitPreview = (mappingId: string, revision: string) =>
  unwrap(invoke<HistoryPreview>("gist_commit_preview", { mappingId, revision }));

export const historyRollbackRemote = (mappingId: string, revision: string) =>
  unwrap(invoke<MappingStatus>("history_rollback_remote", { mappingId, revision }));

// ── 事件 ─────────────────────────────────────────────────────────────

export function onStatusEvent(cb: (event: StatusEvent) => void) {
  return listen<StatusEvent>("sync://status", (ev) => cb(ev.payload));
}
