# Gist Auto Sync

[中文](./README.md)

A Windows desktop tool that keeps local files in sync with files inside GitHub Gists or self-hosted [Opengist](https://github.com/thomiceli/opengist) instances.

## Features (planned, V0.1 in development)

- Multiple accounts for both GitHub Gist and self-hosted Opengist instances
- File-level mapping: one local file ↔ one file inside a gist (by filename); other files in the gist are never touched
- Two-way auto sync: local changes upload after a debounce; remote changes download on a polling interval
- Independent upload/download direction switches per mapping
- Conflict protection: when both sides changed, syncing pauses for a manual choice — never silently overwrites
- Tray-resident, autostart, English/Chinese UI

## Development

```bash
pnpm install
pnpm tauri dev      # run locally
pnpm test           # frontend tests (Vitest)
cd src-tauri && cargo test   # Rust tests
```

## Security Notes

- Access tokens (GitHub PAT / Opengist access token) are stored only in Windows Credential Manager; they never appear in config files or logs.
- Synced content travels through the gist services you configure (GitHub or your own Opengist instance).

## License

MIT
