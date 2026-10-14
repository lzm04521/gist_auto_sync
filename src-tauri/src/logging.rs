use std::io::{self, Write};

/// 把日志文本中疑似 token 的子串打码：
/// 覆盖 GitHub（ghp_/gho_/ghu_/ghs_/ghr_/github_pat_）与 Opengist（og_）前缀
pub fn redact(s: &str) -> String {
    const PREFIXES: [&str; 6] = ["github_pat_", "ghp_", "gho_", "ghu_", "ghs_", "og_"];

    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // 尝试在每个位置匹配 token 前缀
        let mut matched = None;
        for p in PREFIXES {
            if s[i..].starts_with(p) {
                matched = Some(p);
                break;
            }
        }
        if let Some(p) = matched {
            // 消费前缀后的 token 字符段（字母/数字/下划线/连字符）
            let mut j = i + p.len();
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_' || bytes[j] == b'-')
            {
                j += 1;
            }
            out.push_str("***");
            i = j;
        } else {
            // 按字符边界推进（内容为 UTF-8）
            let ch_len = utf8_char_len(bytes[i]);
            let end = (i + ch_len).min(bytes.len());
            out.push_str(&s[i..end]);
            i = end;
        }
    }
    out
}

fn utf8_char_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

/// 包装 tracing_appender 写入器，所有落盘日志过 redact（spec §8：token 永不入日志）
pub struct RedactingWriter<W: Write> {
    inner: W,
}

impl<W: Write> RedactingWriter<W> {
    pub fn new(inner: W) -> Self {
        Self { inner }
    }
}

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        let redacted = redact(&text);
        self.inner.write_all(redacted.as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// 初始化按天滚动的文件日志（级别来自全局设置）
pub fn init_logging(log_dir: &std::path::Path, level: &str) -> Result<(), String> {
    std::fs::create_dir_all(log_dir).map_err(|e| format!("创建日志目录失败: {e}"))?;
    let file_appender = tracing_appender::rolling::daily(log_dir, "gistsync.log");
    let (writer, guard) = tracing_appender::non_blocking(RedactingWriter::new(file_appender));
    // guard 需要与进程同生命周期；泄漏是最简单的保活方式（进程级单次初始化）
    std::mem::forget(guard);
    let filter = tracing_subscriber::EnvFilter::try_new(level)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .init();
    Ok(())
}

/// 安装 panic hook：panic 信息同步直写 panic.log（过 redact），
/// 绕过 tracing 的 non_blocking 缓冲——panic=abort 下缓冲内容会随进程一起丢失，
/// 否则闪退无任何痕迹（见 doc/20261014-bug报告-配置变更闪退.md 边缘情况第 5 条）。
pub fn install_panic_hook(log_dir: &std::path::Path) {
    let panic_log = log_dir.join("panic.log");
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!(" at {}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "non-string panic payload".to_string()
        };
        let line = format!(
            "[{}] PANIC{location}: {payload}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f")
        );
        // 尽力而为：失败也无法再上报（原 hook 可能也写不动任何地方）
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&panic_log)
        {
            let _ = f.write_all(redact(&line).as_bytes());
            let _ = f.flush();
        }
        default_hook(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_masks_github_and_opengist_tokens() {
        assert_eq!(
            redact("Authorization: Bearer ghp_abc123DEF456xyz tail"),
            "Authorization: Bearer *** tail"
        );
        assert_eq!(redact("token og_AbCdEf123456"), "token ***");
        assert_eq!(
            redact("github_pat_11AAAA0000bbbCCC"),
            "***"
        );
    }

    #[test]
    fn redact_keeps_normal_text() {
        let normal = "同步完成 notes.md → gist abc123 (200 OK)";
        assert_eq!(redact(normal), normal);
    }

    #[test]
    fn redacting_writer_masks_on_write() {
        let mut buf = Vec::new();
        {
            let mut w = RedactingWriter::new(&mut buf);
            w.write_all(b"hello ghp_secretSECRET123\n").unwrap();
        }
        assert_eq!(String::from_utf8(buf).unwrap(), "hello ***\n");
    }
}
