import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  Account,
  AccountInput,
  GistSummary,
  GlobalSettings,
  Mapping,
  MappingInput,
  MappingStatus,
  MappingView,
  StatusEvent,
  UserInfo,
} from "../types";

// ── 账号 ─────────────────────────────────────────────────────────────

export const accountList = () => invoke<Account[]>("account_list");

export const accountCreate = (input: AccountInput) =>
  invoke<Account>("account_create", { input });

export const accountUpdate = (id: string, input: AccountInput) =>
  invoke<void>("account_update", { id, input });

export const accountDelete = (id: string, force: boolean) =>
  invoke<void>("account_delete", { id, force });

export const accountTest = (id: string) =>
  invoke<UserInfo>("account_test", { id });

// ── 映射 ─────────────────────────────────────────────────────────────

export const mappingList = () => invoke<MappingView[]>("mapping_list");

export const mappingCreate = (input: MappingInput) =>
  invoke<Mapping>("mapping_create", { input });

export const mappingUpdate = (id: string, input: MappingInput) =>
  invoke<void>("mapping_update", { id, input });

export const mappingDelete = (id: string) =>
  invoke<void>("mapping_delete", { id });

export const mappingSetEnabled = (id: string, enabled: boolean) =>
  invoke<void>("mapping_set_enabled", { id, enabled });

// ── gist 选择器 / 同步操作 ───────────────────────────────────────────

export const gistListForAccount = (accountId: string) =>
  invoke<GistSummary[]>("gist_list_for_account", { accountId });

export const syncManual = (mappingId: string, direction: "upload" | "download") =>
  invoke<MappingStatus>("sync_manual", { mappingId, direction });

export const syncSetPaused = (paused: boolean) =>
  invoke<void>("sync_set_paused", { paused });

export const syncIsPaused = () => invoke<boolean>("sync_is_paused");

// ── 设置 ─────────────────────────────────────────────────────────────

export const settingsGet = () => invoke<GlobalSettings>("settings_get");

export const settingsUpdate = (settings: GlobalSettings) =>
  invoke<void>("settings_update", { settings });

export const appOpenLogDir = () => invoke<void>("app_open_log_dir");

/** 冲突弹窗预览：本地 vs 远端当前内容 */
export interface ConflictPreview {
  local: string | null;
  remote: string | null;
}

export const conflictPreview = (mappingId: string) =>
  invoke<ConflictPreview>("conflict_preview", { mappingId });

// ── 事件 ─────────────────────────────────────────────────────────────

export function onStatusEvent(cb: (event: StatusEvent) => void) {
  return listen<StatusEvent>("sync://status", (ev) => cb(ev.payload));
}
