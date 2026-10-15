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
