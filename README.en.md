# Gist Auto Sync

[中文](./README.md)

[![CI](https://github.com/lzm04521/gist_auto_sync/actions/workflows/ci.yml/badge.svg)](https://github.com/lzm04521/gist_auto_sync/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/lzm04521/gist_auto_sync)](https://github.com/lzm04521/gist_auto_sync/releases)
[![Platform](https://img.shields.io/badge/platform-Windows-0078D6)](https://github.com/lzm04521/gist_auto_sync/releases)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri-FFC131)](https://tauri.app)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](./LICENSE)

A Windows desktop tool that keeps local files in sync with files inside GitHub Gists or self-hosted [Opengist](https://github.com/thomiceli/opengist) instances. Local changes upload automatically after a debounce; remote changes pull back on a polling interval — handy for keeping config files, notes, and code snippets in sync across machines via gists.

**Files in a gist that are not mapped are never touched; conflicts are never silently overwritten.**

## Features

### Sync

- **Multiple accounts** for GitHub Gist and self-hosted Opengist instances, with a "test connection" check
- **File-level mapping**: one local file ↔ one file inside a gist (by filename); multiple local files can map to the same gist
- **Two-way auto sync**: local changes upload after a debounce (default 3s, configurable); remote changes download on a polling interval (default 5 minutes, configurable)
- **Direction switches**: independent upload/download toggles per mapping
- **Conflict protection**: when both sides changed, the mapping pauses for a manual choice (with a side-by-side diff); the overwritten side is archived to local history
- **Live status**: dashboard shows per-mapping status (idle/uploading/downloading/conflict/error/missing) with manual "upload now / download now" actions

### History & Rollback

- Browse two history lines per mapping: local archives and remote gist commits
- Line-level diff between any historical version and the current local content
- One-click rollback: the local file and the remote gist both restore to that version, the sync baseline updates in step (no fake conflicts), and the pre-rollback content is archived too
- The most recent 100 local archives are kept per mapping

### Event Scripts

- Hook custom scripts at four moments per mapping: before/after upload and before/after download — e.g. "format before upload, notify after download"
- Use an external script file (`.ps1` / `.bat` / `.cmd`) or inline script content
- See [Event Scripts](#event-scripts)

### Security & UX

- **In-app auto update**: a new-version prompt on launch, update packages signature-verified; manual check available in Settings
- **Tray-resident**: closing the window minimizes to tray, optional autostart, English/Chinese UI
- **Token safety**: access tokens live only in Windows Credential Manager; never written to config files or logs (log output is token-redacted as a safety net)

## Install

Download the latest NSIS installer (`.exe`) from [Releases](../../releases). Requires Windows 10 or later.

Updates are built in: when a new version is released you get an update prompt, and every update package is signature-verified before installation.

## Getting Started

### 1. Get an access token

- **GitHub**: Settings → Developer settings → Personal access tokens → **Tokens (classic)** → Generate new token with the `gist` scope. [Direct link](https://github.com/settings/tokens)
- **Opengist**: sign in to your instance → Settings → **Access Tokens** → create with `gist:read` and `gist:write`; the token starts with `og_` and is shown only once.

### 2. Add an account

On the "Accounts" page, fill in the service type and access token; use "test connection" to verify the token first.

### 3. Create a mapping

On the "Mappings" page, pick the account and gist, choose the local file and direction toggles — automatic sync starts right after saving.

## How It Works

- The mapping unit is "local file ↔ file inside a gist": one local file pairs with one gist file by filename; **files in a gist that are not mapped are never touched**
- Local changes upload automatically after a debounce (default 3s, configurable)
- The remote side is polled on an interval (default 5 minutes, configurable) and compared against the last sync snapshot; a download only happens when the content actually changed
- When both sides changed, the mapping pauses for a manual choice (side-by-side diff) — **never silently overwrites**; the overwritten side is archived to local history

## Event Scripts

Each mapping can run a custom script at four moments: before/after upload and before/after download:

- **Source**: an external file (`.ps1` runs via PowerShell, `.bat` / `.cmd` via cmd) or inline script content
- **Context**: injected as environment variables — `GISTSYNC_EVENT` (preUpload / postUpload / preDownload / postDownload), `GISTSYNC_LOCAL_PATH` (absolute local file path), `GISTSYNC_MAPPING_NAME`, `GISTSYNC_TRIGGER` (auto / manual)
- **Failure behavior**: each hook can be configured to block the sync action on failure (e.g. a failed pre-upload script cancels that upload); failure details (including the stdout/stderr tail) go to the log
- **Execution**: silent background execution without a window; 60-second timeout (killed and treated as failure on timeout)

## Development

### Prerequisites

- Node.js 22+ and pnpm 11
- Rust stable toolchain
- Windows 10+ (the project only ships a Windows NSIS installer)

### Common Commands

```bash
pnpm install
pnpm tauri dev      # run locally
pnpm test           # frontend tests (Vitest)
cd src-tauri && cargo test   # Rust tests
pnpm tauri build    # build the NSIS installer
```

Integration tests against the real GitHub API are skipped by default; opt in with:

```bash
GISTSYNC_E2E_TOKEN=ghp_xxx cargo test --manifest-path src-tauri/Cargo.toml e2e -- --ignored
```

### CI

Every commit and PR runs CI (`.github/workflows/ci.yml`): `pnpm test` → `pnpm build` → `cargo test` → `cargo clippy -- -D warnings`; all must pass.

## Project Structure

```
src/           React 19 frontend (Vite + TypeScript): pages, components, i18n resources
src-tauri/     Tauri 2 Rust host: IPC commands, gist API abstraction, sync engine, config & logging
```

## Security Notes

- Access tokens (GitHub PAT / Opengist access token) are stored only in Windows Credential Manager and never appear in config files or logs; a log-layer redaction pass masks token-shaped strings as a safety net.
- Synced content travels through the gist services you configure (GitHub or your own Opengist instance). No third-party telemetry.

## Contributing

Issues and pull requests are welcome. For feature changes, please describe the problem and your proposal in an issue first before diving into code.

## License

[MIT](./LICENSE)
