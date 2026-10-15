/** 与 src-tauri DTO 镜像（全部 camelCase） */

export type ProviderKind = "github" | "opengist";

export interface Account {
  id: string;
  name: string;
  provider: ProviderKind;
  baseUrl: string;
  tokenRef: string;
}

export interface Mapping {
  id: string;
  name: string;
  localPath: string;
  accountId: string;
  gistId: string;
  gistFilename: string;
  uploadEnabled: boolean;
  downloadEnabled: boolean;
  enabled: boolean;
}

export interface GlobalSettings {
  debounceSeconds: number;
  pollIntervalMinutes: number;
  autoStart: boolean;
  logLevel: string;
  language: string;
  autoCheckUpdate: boolean;
}

export interface UserInfo {
  login: string;
  name: string | null;
}

export interface GistSummary {
  id: string;
  description: string | null;
  /** Opengist 的 gist 标题；GitHub 恒 null */
  title: string | null;
  updatedAt: string;
  files: string[];
}

export type MappingStatus =
  | "idle"
  | "uploading"
  | "downloading"
  | "conflict"
  | "error"
  | "missing";

export interface MappingView {
  mapping: Mapping;
  status: MappingStatus;
  message: string | null;
  lastSyncedAt: string | null;
}

export interface StatusEvent {
  mappingId: string;
  status: MappingStatus;
  message: string | null;
  lastSyncedAt: string | null;
}

export interface AccountInput {
  name: string;
  provider: ProviderKind;
  baseUrl: string;
  token: string;
}

export interface MappingInput {
  name: string;
  localPath: string;
  accountId: string;
  gistId: string;
  gistFilename: string;
  uploadEnabled: boolean;
  downloadEnabled: boolean;
  enabled: boolean;
}

// ── 历史（V0.3）────────────────────────────────────────────────────────

export type HistorySide = "local" | "remote";

export type HistoryReason = "download" | "upload" | "rollback" | "legacy";

export interface HistoryEntry {
  ts: string;
  side: HistorySide;
  reason: HistoryReason;
  file: string;
  sizeBytes: number;
}

export interface GistCommit {
  revision: string;
  committedAt: string;
}

export interface HistoryPreview {
  historical: string;
  currentLocal: string;
}

// ── 同步操作日志（V0.5）────────────────────────────────────────────────

export type LogTrigger =
  | "localSettled"
  | "poll"
  | "manualUpload"
  | "manualDownload";

export type LogDirection = "upload" | "download";

export type LogOutcome =
  | { kind: "changed"; added: number | null; removed: number | null }
  | { kind: "noChange" }
  | { kind: "conflict" }
  | { kind: "error"; code: string };

export interface SyncLogEntry {
  ts: string;
  mappingId: string;
  /** 显示名快照（映射删除后仍可读） */
  label: string;
  trigger: LogTrigger;
  direction: LogDirection | null;
  outcome: LogOutcome;
}
