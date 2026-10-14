# Gist Auto Sync

[中文](./README.md)

A Windows desktop tool that keeps local files in sync with files inside GitHub Gists or self-hosted [Opengist](https://github.com/thomiceli/opengist) instances.

## Features

- **Multiple accounts** for GitHub Gist and self-hosted Opengist instances, with a "test connection" check
- **File-level mapping**: one local file ↔ one file inside a gist (by filename); multiple local files can map to the same gist; **files in a gist that are not mapped are never touched**
- **Two-way auto sync**: local changes upload after a debounce (default 3s, configurable); remote changes download on a polling interval (default 5 minutes, configurable)
- **Direction switches**: independent upload/download toggles per mapping
- **Conflict protection**: when both sides changed, the mapping pauses for a manual choice (with a side-by-side diff) — **never silently overwrites**; the overwritten side is archived to local history
- **Live status**: dashboard shows per-mapping status (idle/uploading/downloading/conflict/error/missing) with manual "upload now / download now" actions
- **Tray-resident**: closing the window minimizes to tray, optional autostart, English/Chinese UI
- **Security**: access tokens live only in Windows Credential Manager; never written to config files or logs (log output is token-redacted as a safety net)

## Install

Download the latest NSIS installer (`.exe`) from [Releases](../../releases). Requires Windows 10 or later.

## Getting an access token

- **GitHub**: Settings → Developer settings → Personal access tokens → **Tokens (classic)** → Generate new token with the `gist` scope. [Direct link](https://github.com/settings/tokens)
- **Opengist**: sign in to your instance → Settings → **Access Tokens** → create with `gist:read` and `gist:write`; the token starts with `og_` and is shown only once.

## Development

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

## Security Notes

- Access tokens (GitHub PAT / Opengist access token) are stored only in Windows Credential Manager and never appear in config files or logs; a log-layer redaction pass masks token-shaped strings as a safety net.
- Synced content travels through the gist services you configure (GitHub or your own Opengist instance). No third-party telemetry.

## License

MIT
