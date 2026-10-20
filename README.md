# Gist Auto Sync

[English](./README.en.md)

[![CI](https://github.com/lzm04521/gist_auto_sync/actions/workflows/ci.yml/badge.svg)](https://github.com/lzm04521/gist_auto_sync/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/lzm04521/gist_auto_sync)](https://github.com/lzm04521/gist_auto_sync/releases)
[![Platform](https://img.shields.io/badge/platform-Windows-0078D6)](https://github.com/lzm04521/gist_auto_sync/releases)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri-FFC131)](https://tauri.app)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](./LICENSE)

把本地文件与 GitHub Gist / [Opengist](https://github.com/thomiceli/opengist) 的指定 gist 文件保持自动同步的 Windows 桌面工具。本地改动防抖后自动上传 gist，远端变更按周期自动拉回，适合用 gist 在多台机器间同步配置文件、笔记和代码片段。

**映射外的 gist 文件绝不会被改动；冲突时绝不静默覆盖。**

## 功能

### 同步

- **多账号**：GitHub Gist 与自建 Opengist 实例均可配置多个账号，配置时可"测试连接"
- **文件级映射**：一个本地文件 ↔ 一个 gist 内文件（按文件名配对），每个 gist 可映射多个文件
- **双向自动同步**：本地改动防抖后（默认 3 秒，可调）自动上传；远端变更按轮询周期（默认 5 分钟，可调）自动下载
- **方向开关**：每条映射可独立开关"本地 → 远端"、"远端 → 本地"
- **冲突保护**：本地与远端同时变更时暂停该映射并提示，人工选择保留哪边（带双栏 diff），被覆盖一侧的旧内容自动存入本地历史
- **状态可视**：仪表盘实时显示每条映射状态（正常/上传中/下载中/冲突/错误/文件缺失），支持手动"立即上传/立即下载"

### 历史与回滚

- 每条映射可浏览两条历史线：本地归档历史 + 远端 gist commits 历史
- 任一历史版本可与当前本地内容做行级 diff 对比
- 一键回滚：本地文件与远端 gist 同时恢复到该版本，同步基准一并更新，不产生假冲突，回滚前的内容同样被归档
- 每条映射自动保留最近 100 条本地归档

### 事件脚本

- 每条映射可在四个时机挂自定义脚本：上传前、上传后、下载前、下载后，满足"上传前格式化、下载后通知"等诉求
- 支持外部脚本文件（`.ps1` / `.bat` / `.cmd`）或内联脚本内容
- 详见[事件脚本](#事件脚本)

### 安全与体验

- **应用内自动更新**：新版本发布后启动时提示更新，更新包带签名校验；也可在设置页手动检查
- **托盘常驻**：关闭窗口最小化到托盘，开机自启（可选），中英文界面
- **令牌安全**：访问令牌只存 Windows 凭据管理器，配置文件与日志中不落明文（日志输出带 token 打码兜底）

## 安装

从 [Releases](../../releases) 下载最新的 NSIS 安装包（`.exe`）安装即可。需要 Windows 10 及以上。

应用内置自动更新：新版本发布后会收到更新提示，更新包均通过签名校验后安装。

## 快速上手

### 1. 获取访问令牌

- **GitHub**：Settings → Developer settings → Personal access tokens → **Tokens (classic)** → Generate new token，勾选 `gist` 权限。[直达链接](https://github.com/settings/tokens)
- **Opengist**：登录你的实例 → 设置 → **Access Tokens** → 创建（勾选 `gist:read`、`gist:write`），令牌 `og_` 开头，仅显示一次。

### 2. 添加账号

在应用"账号"页填入服务类型与访问令牌，可先"测试连接"验证令牌有效。

### 3. 创建映射

在"映射"页选择账号与 gist，指定要同步的本地文件与方向开关，保存后即开始自动同步。

## 工作原理

- 映射单位是"本地文件 ↔ gist 内文件"：一个本地文件按文件名对应 gist 内一个文件，**映射外的 gist 文件绝不会被改动**
- 本地改动经防抖（默认 3 秒，可调）后自动上传
- 远端按轮询周期（默认 5 分钟，可调）检查，与上次同步快照比对，内容确有变化才下载
- 本地与远端同时变更时暂停该映射，人工选择保留哪边（双栏 diff），**绝不静默覆盖**；被覆盖一侧的旧内容自动存入本地历史

## 事件脚本

每条映射可在上传前、上传后、下载前、下载后四个时机执行自定义脚本：

- **脚本来源**：外部文件（`.ps1` 用 PowerShell 执行，`.bat` / `.cmd` 用 cmd 执行）或内联脚本内容
- **上下文**：以环境变量注入脚本 —— `GISTSYNC_EVENT`（preUpload / postUpload / preDownload / postDownload）、`GISTSYNC_LOCAL_PATH`（本地文件绝对路径）、`GISTSYNC_MAPPING_NAME`（映射名）、`GISTSYNC_TRIGGER`（auto / manual）
- **失败行为**：每个钩子可单独配置"失败时是否阻断本次同步动作"（如上传前脚本失败即取消本次上传），失败详情（含 stdout/stderr 尾部）记录进日志
- **执行方式**：后台静默执行不弹窗，超时 60 秒（超时强制结束并按失败处理）

## 开发

### 环境要求

- Node.js 22+ 与 pnpm 11
- Rust stable 工具链
- Windows 10+（项目仅产出 Windows NSIS 安装包）

### 常用命令

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

### CI

每次提交与 PR 运行 CI（`.github/workflows/ci.yml`）：`pnpm test` → `pnpm build` → `cargo test` → `cargo clippy -- -D warnings`，全过才算绿。

## 项目结构

```
src/           React 19 前端（Vite + TypeScript）：页面、组件、中英文资源
src-tauri/     Tauri 2 Rust 主进程：IPC 命令、gist API 抽象、同步引擎、配置与日志
```

## 安全说明

- 访问令牌（GitHub PAT / Opengist Access Token）仅存放于 Windows 凭据管理器，不写入任何配置文件或日志；日志层另有 token 模式打码兜底。
- 同步内容经由你所配置的 gist 服务（GitHub 或你的 Opengist 实例）传输，本工具不做任何第三方上报。

## 贡献

欢迎提交 Issue 与 Pull Request。功能类改动建议先在 Issue 里说明问题与方案，确认方向后再动手。

## License

[MIT](./LICENSE)
