use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::Watcher;
use tokio::sync::mpsc;

/// 滑动防抖：窗口期内每来一个新事件就重置计时，静默期满发送最终路径（spec §6.1）
pub async fn debounce_stream(
    mut rx: mpsc::Receiver<PathBuf>,
    window: Duration,
    tx: mpsc::Sender<PathBuf>,
) {
    while let Some(first) = rx.recv().await {
        let mut last = first;
        let mut deadline = tokio::time::Instant::now() + window;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Some(path)) => {
                    last = path;
                    // 滑动窗口：新事件重置静默计时
                    deadline = tokio::time::Instant::now() + window;
                }
                Ok(None) => break, // 上游关闭，立即结算
                Err(_elapsed) => break, // 静默期满
            }
        }
        let _ = tx.send(last).await;
    }
}

/// notify 事件回调的过滤逻辑：只放行被监听文件集合内的路径（Windows 大小写不敏感比较）
fn make_filter(watched: &[PathBuf]) -> HashSet<String> {
    watched
        .iter()
        .map(|p| path_key(p))
        .collect()
}

fn path_key(p: &Path) -> String {
    p.to_string_lossy().to_ascii_lowercase()
}

/// 文件监听器：监听各映射文件的父目录（去重），事件过滤+防抖后输出"稳定下来"的文件路径。
/// Remove 事件也放行——由引擎按 MissingLocal 处理（spec §6.3）。
pub struct FileWatcher {
    _watcher: notify::RecommendedWatcher,
}

impl FileWatcher {
    /// 启动监听。需要在 tokio 运行时上下文中调用（内部 spawn 防抖任务）。
    pub fn start(
        watched_files: Vec<PathBuf>,
        window: Duration,
        out: mpsc::Sender<PathBuf>,
    ) -> notify::Result<Self> {
        let filter = make_filter(&watched_files);
        let dirs: HashSet<String> = watched_files
            .iter()
            .filter_map(|p| p.parent().map(path_key))
            .collect();

        let (event_tx, event_rx) = mpsc::channel::<PathBuf>(256);
        let mut watcher = notify::recommended_watcher(
            move |res: Result<notify::Event, notify::Error>| {
                let Ok(ev) = res else { return };
                if !matches!(
                    ev.kind,
                    notify::EventKind::Create(_) | notify::EventKind::Modify(_) | notify::EventKind::Remove(_)
                ) {
                    return;
                }
                for p in ev.paths {
                    if filter.contains(&path_key(&p)) {
                        // notify 回调运行在独立线程，用 blocking_send 送入 tokio 通道
                        let _ = event_tx.blocking_send(p);
                    }
                }
            },
        )?;

        // 单个目录监听失败（如离线的移动盘/网络盘）只跳过并告警，不整体失败
        let mut watched_count = 0usize;
        let total = dirs.len();
        for dir in dirs {
            let dir = PathBuf::from(dir);
            match watcher.watch(&dir, notify::RecursiveMode::NonRecursive) {
                Ok(()) => watched_count += 1,
                Err(e) => tracing::warn!("监听目录失败（已跳过）: {dir:?} — {e}"),
            }
        }
        if total > 0 && watched_count == 0 {
            return Err(notify::Error::new(notify::ErrorKind::WatchNotFound));
        }

        let debounce_out = out;
        tokio::spawn(debounce_stream(event_rx, window, debounce_out));

        Ok(Self { _watcher: watcher })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[tokio::test(start_paused = true)]
    async fn burst_within_window_settles_once_at_end() {
        let (in_tx, in_rx) = mpsc::channel(16);
        let (out_tx, mut out_rx) = mpsc::channel(16);
        tokio::spawn(debounce_stream(in_rx, Duration::from_secs(3), out_tx));

        in_tx.send(p("C:/a.md")).await.unwrap();
        tokio::time::advance(Duration::from_secs(1)).await;
        in_tx.send(p("C:/a.md")).await.unwrap();
        tokio::time::advance(Duration::from_millis(500)).await;
        in_tx.send(p("C:/a.md")).await.unwrap();

        // 第二次事件重置窗口后静默 3s（自动推进），只应收到一条
        let got = out_rx.recv().await.unwrap();
        assert_eq!(got, p("C:/a.md"));
        // 再无第二条
        assert!(
            tokio::time::timeout(Duration::from_secs(5), out_rx.recv())
                .await
                .is_err()
        );
    }

    // ── 终审修复通道：单个不可监听目录跳过而非整体失败 ──
    #[tokio::test]
    async fn watcher_start_skips_unwatchable_dir_but_keeps_rest() {
        let (out_tx, _out_rx) = mpsc::channel(16);
        let ghost = std::env::temp_dir().join("gistsync-ghost-dir").join("f.md");
        let real = tempfile::tempdir().unwrap();
        let real_file = real.path().join("a.md");
        std::fs::write(&real_file, "x").unwrap();

        let result = FileWatcher::start(vec![ghost, real_file], Duration::from_secs(1), out_tx);
        assert!(result.is_ok(), "单个失败目录应被跳过");
    }

    #[tokio::test]
    async fn watcher_start_all_dirs_unwatchable_errors() {
        let (out_tx, _out_rx) = mpsc::channel(16);
        let ghost = std::env::temp_dir().join("gistsync-ghost-dir2").join("f.md");
        let result = FileWatcher::start(vec![ghost], Duration::from_secs(1), out_tx);
        assert!(result.is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn events_apart_settle_separately() {
        let (in_tx, in_rx) = mpsc::channel(16);
        let (out_tx, mut out_rx) = mpsc::channel(16);
        tokio::spawn(debounce_stream(in_rx, Duration::from_secs(2), out_tx));

        in_tx.send(p("C:/a.md")).await.unwrap();
        let first = out_rx.recv().await.unwrap();
        assert_eq!(first, p("C:/a.md"));

        in_tx.send(p("C:/b.md")).await.unwrap();
        let second = out_rx.recv().await.unwrap();
        assert_eq!(second, p("C:/b.md"));
    }
}
