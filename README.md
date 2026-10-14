# Gist Auto Sync

[English](./README.en.md)

把本地文件与 GitHub Gist / [Opengist](https://github.com/thomiceli/opengist) 的指定 gist 文件保持自动同步的 Windows 桌面工具。

## 功能

- **多账号**：GitHub Gist 与自建 Opengist 实例均可配置多个账号，配置时可"测试连接"
- **文件级映射**：一个本地文件 ↔ 一个 gist 内文件（按文件名），每个 gist 可映射多个文件；**映射外的 gist 文件绝不会被改动**
- **双向自动同步**：本地改动防抖后（默认 3 秒，可调）自动上传；远端变更按轮询周期（默认 5 分钟，可调）自动下载
- **方向开关**：每条映射可独立开关"本地 → 远端"、"远端 → 本地"
- **冲突保护**：本地与远端同时变更时暂停该映射并提示，人工选择保留哪边（带双栏 diff），**绝不静默覆盖**；被覆盖一侧的旧内容自动存入本地历史
- **状态可视**：仪表盘实时显示每条映射状态（正常/上传中/下载中/冲突/错误/文件缺失），支持手动"立即上传/立即下载"
- **托盘常驻**：关闭窗口最小化到托盘，开机自启（可选），中英文界面
- **安全**：访问令牌只存 Windows 凭据管理器，配置文件与日志中不落明文（日志输出带 token 打码兜底）

## 安装

从 [Releases](../../releases) 下载最新的 NSIS 安装包（`.exe`）安装即可。需要 Windows 10 及以上。

## 获取访问令牌

- **GitHub**：Settings → Developer settings → Personal access tokens → **Tokens (classic)** → Generate new token，勾选 `gist` 权限。[直达链接](https://github.com/settings/tokens)
- **Opengist**：登录你的实例 → 设置 → **Access Tokens** → 创建（勾选 `gist:read`、`gist:write`），令牌 `og_` 开头，仅显示一次。

## 开发

```bash
pnpm install
pnpm tauri dev      # 本地开发运行
pnpm test           # 前端测试（Vitest）
cd src-tauri && cargo test   # Rust 测试
pnpm tauri build    # 打 NSIS 安装包
```

真实 GitHub API 的集成测试默认跳过，需要时：

```bash
GISTSYNC_E2E_TOKEN=ghp_xxx cargo test --manifest-path src-tauri/Cargo.toml e2e -- --ignored
```

## 安全说明

- 访问令牌（GitHub PAT / Opengist Access Token）仅存放于 Windows 凭据管理器，不写入任何配置文件或日志；日志层另有 token 模式打码兜底。
- 同步内容经由你所配置的 gist 服务（GitHub 或你的 Opengist 实例）传输，本工具不做任何第三方上报。

## License

MIT
