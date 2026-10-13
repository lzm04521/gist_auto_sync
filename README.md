# Gist Auto Sync

[English](./README.en.md)

把本地文件与 GitHub Gist / [Opengist](https://github.com/thomiceli/opengist) 的指定 gist 文件保持自动同步的 Windows 桌面工具。

## 功能（规划中，V0.1 开发中）

- 多账号管理：GitHub Gist 与自建 Opengist 实例，均可配置多个账号
- 文件级映射：一个本地文件 ↔ 一个 gist 内文件（按文件名），不影响 gist 内其他文件
- 双向自动同步：本地改动防抖后自动上传；远端变更周期轮询自动下载
- 每条映射独立的上传/下载方向开关
- 冲突保护：本地与远端同时变更时暂停并提示人工选择，绝不静默覆盖
- 托盘常驻、开机自启、中英文界面

## 开发

```bash
pnpm install
pnpm tauri dev      # 本地开发运行
pnpm test           # 前端测试（Vitest）
cd src-tauri && cargo test   # Rust 测试
```

## 安全说明

- 访问令牌（GitHub PAT / Opengist Access Token）仅存放于 Windows 凭据管理器，不写入任何配置文件或日志。
- 同步内容经由你所配置的 gist 服务（GitHub 或你的 Opengist 实例）传输。

## License

MIT
