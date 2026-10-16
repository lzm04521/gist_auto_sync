//! 事件脚本执行器：静默执行 ps1/bat 脚本（CREATE_NO_WINDOW + 超时 + 环境变量注入 + 内联物化）。

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::model::{EventHook, HookMode, InlineKind};

/// 公共入口固定超时；测试经 `run_script` 注入短值
const HOOK_TIMEOUT: Duration = Duration::from_secs(60);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// 失败详情里保留的输出尾部长度（字符）
const DETAIL_TAIL: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HookEvent {
    PreUpload,
    PostUpload,
    PreDownload,
    PostDownload,
}

impl HookEvent {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            HookEvent::PreUpload => "preUpload",
            HookEvent::PostUpload => "postUpload",
            HookEvent::PreDownload => "preDownload",
            HookEvent::PostDownload => "postDownload",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HookTrigger {
    Auto,
    Manual,
}

impl HookTrigger {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            HookTrigger::Auto => "auto",
            HookTrigger::Manual => "manual",
        }
    }
}

/// 执行上下文：引擎侧组装，注入环境变量与工作目录
pub(crate) struct HookContext {
    pub local_path: String,
    pub mapping_name: String,
    pub trigger: HookTrigger,
}

/// 执行结果：失败信息全部封装在此，由调用方按 block_on_failure 决定语义；永不 Err 上抛
pub(crate) struct HookOutcome {
    pub ok: bool,
    pub timed_out: bool,
    pub exit_code: Option<i32>,
    pub elapsed: Duration,
    /// 失败原因 + stdout/stderr 尾部（成功时为空）
    pub detail: String,
}

/// 执行一个已配置的钩子（60s 超时）
pub(crate) async fn run_hook(hook: &EventHook, event: HookEvent, ctx: &HookContext) -> HookOutcome {
    match hook.mode {
        HookMode::File => {
            let script = Path::new(&hook.script_path);
            tracing::info!(
                "[{}] 钩子 {} 开始: file:{}",
                ctx.mapping_name,
                event.as_str(),
                hook.script_path
            );
            let out = run_script(script, event, ctx, HOOK_TIMEOUT).await;
            log_result(ctx, event, &out);
            out
        }
        HookMode::Inline => {
            let kind = match hook.inline_kind {
                InlineKind::PowerShell => "ps1",
                InlineKind::Bat => "bat",
            };
            tracing::info!("[{}] 钩子 {} 开始: inline:{kind}", ctx.mapping_name, event.as_str());
            let out = match materialize_inline(hook, event) {
                Ok(path) => {
                    let out = run_script(&path, event, ctx, HOOK_TIMEOUT).await;
                    // 执行完（含超时/失败）best-effort 清理临时文件
                    if let Err(e) = std::fs::remove_file(&path) {
                        tracing::debug!("[{}] 清理内联脚本临时文件失败（不影响同步）: {e}", ctx.mapping_name);
                    }
                    out
                }
                Err(e) => HookOutcome {
                    ok: false,
                    timed_out: false,
                    exit_code: None,
                    elapsed: Duration::ZERO,
                    detail: format!("内联脚本物化失败: {e}"),
                },
            };
            log_result(ctx, event, &out);
            out
        }
    }
}

/// 内联脚本物化到系统临时目录（每次执行重写，保证与配置当前值一致）。
/// ps1 恒写 UTF-8 带 BOM（PS 5.1 无 BOM 按系统代码页解析，中文会乱码）；
/// 换行统一 LF→CRLF（先拆 \r\n 再拼，幂等不产生 \r\r\n；cmd 批处理 LF-only 有边缘坑）。
fn materialize_inline(hook: &EventHook, event: HookEvent) -> std::io::Result<PathBuf> {
    let ext = match hook.inline_kind {
        InlineKind::PowerShell => "ps1",
        InlineKind::Bat => "bat",
    };
    let name = format!("gistsync-{}-{}.{}", event.as_str(), uuid::Uuid::new_v4().simple(), ext);
    let path = std::env::temp_dir().join(name);
    let normalized = hook.script_content.replace("\r\n", "\n").replace('\n', "\r\n");
    let mut bytes: Vec<u8> = Vec::with_capacity(normalized.len() + 3);
    if matches!(hook.inline_kind, InlineKind::PowerShell) {
        bytes.extend_from_slice(b"\xEF\xBB\xBF");
    }
    bytes.extend_from_slice(normalized.as_bytes());
    std::fs::write(&path, bytes)?;
    Ok(path)
}

async fn run_script(script: &Path, event: HookEvent, ctx: &HookContext, timeout: Duration) -> HookOutcome {
    let started = std::time::Instant::now();
    let ext = script
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let (program, args): (&str, Vec<String>) = if ext == "ps1" {
        (
            "powershell.exe",
            vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-File".into(),
                script.to_string_lossy().into_owned(),
            ],
        )
    } else {
        ("cmd.exe", vec!["/C".into(), script.to_string_lossy().into_owned()])
    };

    let mut cmd = tokio::process::Command::new(program);
    cmd.args(&args)
        .creation_flags(CREATE_NO_WINDOW)
        .env("GISTSYNC_EVENT", event.as_str())
        .env("GISTSYNC_LOCAL_PATH", &ctx.local_path)
        .env("GISTSYNC_MAPPING_NAME", &ctx.mapping_name)
        .env("GISTSYNC_TRIGGER", ctx.trigger.as_str())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // 超时分支 drop 掉 wait future 时连带杀进程
        .kill_on_drop(true);
    if let Some(dir) = Path::new(&ctx.local_path).parent().filter(|p| !p.as_os_str().is_empty()) {
        cmd.current_dir(dir);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return HookOutcome {
                ok: false,
                timed_out: false,
                exit_code: None,
                elapsed: started.elapsed(),
                detail: format!("启动脚本失败（{program}）: {e}"),
            }
        }
    };

    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let ok = output.status.success();
            let exit_code = output.status.code();
            let detail = if ok {
                String::new()
            } else {
                let combined = format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                format!("exit={exit_code:?}: {}", tail(&combined))
            };
            HookOutcome { ok, timed_out: false, exit_code, elapsed: started.elapsed(), detail }
        }
        Ok(Err(e)) => HookOutcome {
            ok: false,
            timed_out: false,
            exit_code: None,
            elapsed: started.elapsed(),
            detail: format!("等待脚本退出失败: {e}"),
        },
        Err(_) => HookOutcome {
            ok: false,
            timed_out: true,
            exit_code: None,
            elapsed: started.elapsed(),
            detail: format!("执行超时（{}s），已终止脚本", timeout.as_secs()),
        },
    }
}

fn tail(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= DETAIL_TAIL {
        s.to_string()
    } else {
        chars[chars.len() - DETAIL_TAIL..].iter().collect()
    }
}

fn log_result(ctx: &HookContext, event: HookEvent, out: &HookOutcome) {
    if out.ok {
        tracing::info!(
            "[{}] 钩子 {} 完成（exit {:?}，耗时 {:.1}s）",
            ctx.mapping_name,
            event.as_str(),
            out.exit_code,
            out.elapsed.as_secs_f64()
        );
    } else if out.timed_out {
        tracing::warn!("[{}] 钩子 {} 超时终止: {}", ctx.mapping_name, event.as_str(), out.detail);
    } else {
        tracing::warn!("[{}] 钩子 {} 失败: {}", ctx.mapping_name, event.as_str(), out.detail);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    fn file_hook(path: &Path) -> crate::model::EventHook {
        crate::model::EventHook {
            mode: crate::model::HookMode::File,
            script_path: path.to_string_lossy().to_string(),
            ..crate::model::EventHook::default()
        }
    }

    fn ctx(local: &Path) -> HookContext {
        HookContext {
            local_path: local.to_string_lossy().to_string(),
            mapping_name: "测试映射".into(),
            trigger: HookTrigger::Auto,
        }
    }

    fn write_script(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, content).unwrap();
        p
    }

    /// 成功脚本：把事件环境变量写到 <本地文件>.marker（同时验证环境变量注入）
    const MARKER_PS1: &str =
        "Set-Content -Path ($env:GISTSYNC_LOCAL_PATH + '.marker') -Value $env:GISTSYNC_EVENT";

    #[tokio::test]
    async fn ps1_success_writes_marker_with_env() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("a.md");
        fs::write(&local, "x").unwrap();
        let script = write_script(dir.path(), "ok.ps1", MARKER_PS1);
        let out = run_hook(&file_hook(&script), HookEvent::PreUpload, &ctx(&local)).await;
        assert!(out.ok, "detail: {}", out.detail);
        assert_eq!(out.exit_code, Some(0));
        let marker = fs::read_to_string(dir.path().join("a.md.marker")).unwrap();
        assert_eq!(marker.trim(), "preUpload");
    }

    #[tokio::test]
    async fn ps1_failure_exit_code_and_stderr_tail() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("a.md");
        fs::write(&local, "x").unwrap();
        let script = write_script(dir.path(), "fail.ps1", "Write-Error boom; exit 1");
        let out = run_hook(&file_hook(&script), HookEvent::PostUpload, &ctx(&local)).await;
        assert!(!out.ok);
        assert_eq!(out.exit_code, Some(1));
        assert!(out.detail.contains("boom"), "detail 应含 stderr 尾部: {}", out.detail);
    }

    #[tokio::test]
    async fn bat_failure_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("a.md");
        fs::write(&local, "x").unwrap();
        let script = write_script(dir.path(), "fail.bat", "@echo off\r\necho boom 1>&2\r\nexit /b 1");
        let out = run_hook(&file_hook(&script), HookEvent::PreDownload, &ctx(&local)).await;
        assert!(!out.ok);
        assert_eq!(out.exit_code, Some(1));
        assert!(out.detail.contains("boom"), "detail: {}", out.detail);
    }

    #[tokio::test]
    async fn timeout_kills_process() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("a.md");
        fs::write(&local, "x").unwrap();
        let script = write_script(dir.path(), "slow.ps1", "Start-Sleep -Seconds 30");
        let started = std::time::Instant::now();
        let out = run_script(&script, HookEvent::PreUpload, &ctx(&local), Duration::from_millis(200)).await;
        assert!(out.timed_out);
        assert!(!out.ok);
        assert!(out.elapsed.as_secs() < 5, "超时后应很快返回，实际 {:?}", out.elapsed);
        assert!(started.elapsed().as_secs() < 10);
    }

    #[tokio::test]
    async fn missing_script_is_failed_outcome_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("a.md");
        fs::write(&local, "x").unwrap();
        let script = dir.path().join("not-exist.ps1");
        let out = run_hook(&file_hook(&script), HookEvent::PostDownload, &ctx(&local)).await;
        assert!(!out.ok);
        assert!(!out.detail.is_empty(), "失败原因要写进 detail");
    }

    #[tokio::test]
    async fn runs_script_in_path_with_spaces_and_unicode() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("目录 空格");
        fs::create_dir(&sub).unwrap();
        let local = dir.path().join("a.md");
        fs::write(&local, "x").unwrap();
        let script = write_script(&sub, "ok.ps1", MARKER_PS1);
        let out = run_hook(&file_hook(&script), HookEvent::PostUpload, &ctx(&local)).await;
        assert!(out.ok, "detail: {}", out.detail);
        assert!(dir.path().join("a.md.marker").exists());
    }

    #[tokio::test]
    async fn workdir_is_local_file_dir() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("a.md");
        fs::write(&local, "x").unwrap();
        // 脚本放子目录，marker 相对 cwd（= 本地文件所在目录）生成才能证明工作目录正确
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        let script = write_script(&sub, "cwd.ps1", "Set-Content -Path marker.txt -Value (Get-Location).Path");
        let out = run_hook(&file_hook(&script), HookEvent::PreUpload, &ctx(&local)).await;
        assert!(out.ok, "detail: {}", out.detail);
        let marker = fs::read_to_string(dir.path().join("marker.txt")).unwrap();
        assert_eq!(
            fs::canonicalize(dir.path()).unwrap(),
            fs::canonicalize(Path::new(marker.trim())).unwrap()
        );
    }

    // ── Inline 物化（Task 3）────────────────────────────────────────

    fn inline_hook(kind: crate::model::InlineKind, content: &str) -> crate::model::EventHook {
        crate::model::EventHook {
            mode: crate::model::HookMode::Inline,
            script_content: content.into(),
            inline_kind: kind,
            ..crate::model::EventHook::default()
        }
    }

    /// 断言 temp 下指定事件前缀的内联临时文件数量
    fn temp_count(event: &str) -> usize {
        fs::read_dir(std::env::temp_dir())
            .map(|rd| {
                rd.flatten()
                    .filter(|e| {
                        e.file_name().to_string_lossy().starts_with(&format!("gistsync-{event}-"))
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    #[tokio::test]
    async fn inline_ps1_chinese_roundtrip() {
        // 输出用显式 UTF-8 WriteAllText，隔离系统区域对 Set-Content 默认编码的影响；
        // 本测试真正验证的是：含中文的 ps1 临时文件被 PowerShell 按 UTF-8（BOM）正确解析
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("a.md");
        fs::write(&local, "x").unwrap();
        let hook = inline_hook(
            crate::model::InlineKind::PowerShell,
            "$enc = New-Object System.Text.UTF8Encoding($false)\n[System.IO.File]::WriteAllText(($env:GISTSYNC_LOCAL_PATH + '.zh'), '中文内容测试', $enc)",
        );
        let out = run_hook(&hook, HookEvent::PostUpload, &ctx(&local)).await;
        assert!(out.ok, "detail: {}", out.detail);
        let written = fs::read_to_string(dir.path().join("a.md.zh")).unwrap();
        assert_eq!(written, "中文内容测试");
    }

    #[test]
    fn inline_crlf_content_stays_crlf_not_crcr() {
        let body = |bytes: &[u8]| -> Vec<u8> {
            // 剥掉 ps1 的 UTF-8 BOM 后比较
            if bytes.starts_with(b"\xEF\xBB\xBF") {
                bytes[3..].to_vec()
            } else {
                bytes.to_vec()
            }
        };
        // LF 输入 → 写盘归一为 CRLF（cmd 批处理 LF-only 有 goto/标签边缘坑）
        let lf = inline_hook(crate::model::InlineKind::PowerShell, "a\nb\nc\n");
        let path = materialize_inline(&lf, HookEvent::PreUpload).unwrap();
        let bytes = body(&fs::read(&path).unwrap());
        fs::remove_file(&path).unwrap();
        assert_eq!(bytes, b"a\r\nb\r\nc\r\n", "LF 应归一为 CRLF");
        // 已含 CRLF 的输入不得变成 CRCR（归一必须幂等）
        let crlf = inline_hook(crate::model::InlineKind::PowerShell, "a\r\nb\r\nc\r\n");
        let path = materialize_inline(&crlf, HookEvent::PostUpload).unwrap();
        let bytes = body(&fs::read(&path).unwrap());
        fs::remove_file(&path).unwrap();
        assert_eq!(bytes, b"a\r\nb\r\nc\r\n", "CRLF 不得归一成 CRCR");
    }

    #[tokio::test]
    async fn inline_temp_file_cleaned_after_run() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("a.md");
        fs::write(&local, "x").unwrap();
        // 成功路径
        let ok_hook = inline_hook(crate::model::InlineKind::PowerShell, "exit 0");
        let out = run_hook(&ok_hook, HookEvent::PreUpload, &ctx(&local)).await;
        assert!(out.ok, "detail: {}", out.detail);
        assert_eq!(temp_count("preUpload"), 0, "成功执行后临时文件应被清理");
        // 失败路径
        let fail_hook = inline_hook(crate::model::InlineKind::PowerShell, "exit 1");
        let out = run_hook(&fail_hook, HookEvent::PreDownload, &ctx(&local)).await;
        assert!(!out.ok);
        assert_eq!(temp_count("preDownload"), 0, "失败执行后临时文件也应被清理");
    }

    #[tokio::test]
    async fn inline_bat_smoke() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("a.md");
        fs::write(&local, "x").unwrap();
        let hook = inline_hook(crate::model::InlineKind::Bat, "@echo off\nexit /b 0");
        let out = run_hook(&hook, HookEvent::PostDownload, &ctx(&local)).await;
        assert!(out.ok, "detail: {}", out.detail);
    }
}
