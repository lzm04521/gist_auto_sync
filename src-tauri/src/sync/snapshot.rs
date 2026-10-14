use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use super::history::{
    append_entry, list_entries, prune, read_archive, HistoryEntry, HistoryReason,
};

/// 被覆盖的一侧（历史归档用）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Local,
    Remote,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Local => "local",
            Side::Remote => "remote",
        }
    }

    pub fn parse(s: &str) -> Option<Side> {
        match s {
            "local" => Some(Side::Local),
            "remote" => Some(Side::Remote),
            _ => None,
        }
    }
}

/// 同步方向
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncDirection {
    Upload,
    Download,
}

/// 每条映射的同步基准（快照）：以"远端回读内容"为基准（spec §6.5）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotState {
    pub last_synced_hash: String,
    pub last_synced_at: DateTime<Utc>,
    pub last_direction: Option<SyncDirection>,
    pub remote_updated_at: Option<DateTime<Utc>>,
}

/// 快照存储：<root>/<mapping_id>/state.json + 历史文件
pub struct SnapshotStore {
    root: PathBuf,
    /// 最近一次归档使用的毫秒时间戳（保证文件名单调递增不复用：
    /// 仅靠"文件存在"探针，惰性清理删掉旧名后同毫秒探针会复用已用过的名字）
    last_archive_ms: Mutex<Option<i64>>,
}

/// 每映射归档保留上限（固定值，spec §3.3）
const HISTORY_KEEP: usize = 100;

impl SnapshotStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            last_archive_ms: Mutex::new(None),
        }
    }

    /// 读取快照；从未同步过 → None
    pub fn load(&self, mapping_id: &str) -> Option<SnapshotState> {
        let path = self.mapping_dir(mapping_id).join("state.json");
        let text = fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self, mapping_id: &str, state: &SnapshotState) -> std::io::Result<()> {
        let dir = self.mapping_dir(mapping_id);
        fs::create_dir_all(&dir)?;
        let json = serde_json::to_string_pretty(state)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        // 原子写：先临时文件再改名
        let tmp = dir.join("state.json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(tmp, dir.join("state.json"))
    }

    /// 覆盖前归档被覆盖一侧的旧内容：<ts>-{local|remote}.<ext>；
    /// 同时追加 jsonl 元数据并惰性清理超限归档（先文件后 jsonl，追加失败退化为裸归档）
    pub fn archive(
        &self,
        mapping_id: &str,
        side: Side,
        reason: HistoryReason,
        content: &str,
        ext: &str,
    ) -> std::io::Result<PathBuf> {
        let dir = self.mapping_dir(mapping_id);
        fs::create_dir_all(&dir)?;
        let mut last_ms = self
            .last_archive_ms
            .lock()
            .expect("archive ts lock poisoned");
        let mut ts_ms = Utc::now().timestamp_millis().max(last_ms.unwrap_or(i64::MIN));
        let start_ms = ts_ms;
        // 同毫秒碰撞：时间戳 +1ms 重试（上限 1000 次）
        let ts_of = |ms: i64| {
            Utc.timestamp_millis_opt(ms)
                .single()
                .expect("timestamp_millis 始终唯一")
        };
        let path = loop {
            let name = format!(
                "{}-{}.{ext}",
                ts_of(ts_ms).format("%Y%m%d%H%M%S%3f"),
                side.as_str()
            );
            let candidate = dir.join(name);
            if !candidate.exists() {
                break candidate;
            }
            ts_ms += 1;
            if ts_ms - start_ms >= 1000 {
                return Err(std::io::Error::other("归档文件名碰撞重试超限"));
            }
        };
        *last_ms = Some(ts_ms);
        fs::write(&path, content)?;

        let entry = HistoryEntry {
            ts: ts_of(ts_ms),
            side,
            reason,
            file: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            size_bytes: content.len() as u64,
        };
        if let Err(e) = append_entry(&dir, &entry) {
            tracing::warn!("[{mapping_id}] 归档元数据写入失败，退化为裸归档: {e}");
        }
        if let Err(e) = prune(&dir, HISTORY_KEEP) {
            tracing::warn!("[{mapping_id}] 历史归档清理失败: {e}");
        }
        Ok(path)
    }

    /// 删除该映射的整个快照目录（映射删除时）
    pub fn remove_mapping(&self, mapping_id: &str) -> std::io::Result<()> {
        match fs::remove_dir_all(self.mapping_dir(mapping_id)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// 读该映射的归档文件内容（不合法文件名 → InvalidInput，root 不外泄）
    pub fn read_archive(&self, mapping_id: &str, file_name: &str) -> std::io::Result<String> {
        read_archive(&self.mapping_dir(mapping_id), file_name)
    }

    /// 该映射的本地归档列表（jsonl + 裸文件合并，ts 倒序）
    pub fn history_list(&self, mapping_id: &str) -> Vec<HistoryEntry> {
        list_entries(&self.mapping_dir(mapping_id))
    }

    fn mapping_dir(&self, mapping_id: &str) -> PathBuf {
        self.root.join(mapping_id)
    }
}

/// 自写抑制表：引擎下载写盘后登记 (hash, mtime)，
/// watcher 事件命中即忽略，防止"下载→触发上传"回路。
#[derive(Default)]
pub struct SelfWriteGuard {
    inner: Mutex<std::collections::HashMap<String, (String, SystemTime)>>,
}

impl SelfWriteGuard {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, mapping_id: &str, hash: &str, mtime: SystemTime) {
        self.inner
            .lock()
            .expect("self-write guard lock poisoned")
            .insert(mapping_id.to_string(), (hash.to_string(), mtime));
    }

    pub fn should_suppress(&self, mapping_id: &str, hash: &str, mtime: SystemTime) -> bool {
        self.inner
            .lock()
            .expect("self-write guard lock poisoned")
            .get(mapping_id)
            .is_some_and(|(h, m)| h == hash && *m == mtime)
    }

    pub fn clear(&self, mapping_id: &str) {
        self.inner
            .lock()
            .expect("self-write guard lock poisoned")
            .remove(mapping_id);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::sync::history::{is_archive_name, list_entries, HistoryReason};

    #[test]
    fn load_save_roundtrip_creates_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path().to_path_buf());
        assert!(store.load("m1").is_none());

        let state = SnapshotState {
            last_synced_hash: "abc".into(),
            last_synced_at: Utc::now(),
            last_direction: Some(SyncDirection::Upload),
            remote_updated_at: None,
        };
        store.save("m1", &state).unwrap();
        assert_eq!(store.load("m1"), Some(state));
    }

    #[test]
    fn archive_writes_timestamped_side_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path().to_path_buf());
        let path = store
            .archive("m1", Side::Local, HistoryReason::Download, "旧本地内容", "md")
            .unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.ends_with("-local.md"), "name={name}");
        assert!(name.len() > "-local.md".len() + 15, "name={name}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "旧本地内容");

        let remote = store
            .archive("m1", Side::Remote, HistoryReason::Upload, "old remote", "txt")
            .unwrap();
        assert!(remote
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with("-remote.txt"));
    }

    #[test]
    fn archive_appends_jsonl_entry() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path().to_path_buf());
        let path = store
            .archive("m1", Side::Local, HistoryReason::Download, "内容A", "md")
            .unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();

        let listed = list_entries(&store.mapping_dir("m1"));
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].side, Side::Local);
        assert_eq!(listed[0].reason, HistoryReason::Download);
        assert_eq!(listed[0].file, name);
        assert_eq!(listed[0].size_bytes, "内容A".len() as u64);
    }

    #[test]
    fn archive_same_millisecond_names_unique() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path().to_path_buf());
        let mut names = HashSet::new();
        for i in 0..200 {
            let p = store
                .archive("m1", Side::Local, HistoryReason::Download, &format!("c{i}"), "md")
                .unwrap();
            assert!(
                names.insert(p.file_name().unwrap().to_string_lossy().to_string()),
                "归档文件名重复（第 {i} 次）"
            );
        }
        assert_eq!(names.len(), 200);
    }

    #[test]
    fn archive_prunes_to_cap() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path().to_path_buf());
        let mut first = None;
        let mut last = None;
        for i in 0..200 {
            let p = store
                .archive("m1", Side::Local, HistoryReason::Download, &format!("c{i}"), "md")
                .unwrap();
            let n = p.file_name().unwrap().to_string_lossy().to_string();
            if i == 0 {
                first = Some(n.clone());
            }
            last = Some(n);
        }
        let count = fs::read_dir(store.mapping_dir("m1"))
            .unwrap()
            .filter(|e| is_archive_name(&e.as_ref().unwrap().file_name().to_string_lossy()))
            .count();
        assert_eq!(count, 100, "归档保留上限应为 100");
        assert!(!store.mapping_dir("m1").join(first.unwrap()).exists(), "最旧的归档应被清理");
        assert!(store.mapping_dir("m1").join(last.unwrap()).exists(), "最新的归档应保留");
    }

    #[test]
    fn remove_mapping_deletes_dir() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path().to_path_buf());
        store.archive("m1", Side::Local, HistoryReason::Download, "x", "md").unwrap();
        store.remove_mapping("m1").unwrap();
        assert!(!store.mapping_dir("m1").exists());
        // 重复删除不报错
        store.remove_mapping("m1").unwrap();
    }

    #[test]
    fn guard_register_suppress_clear() {
        let guard = SelfWriteGuard::new();
        let now = SystemTime::now();
        guard.register("m1", "hash-a", now);

        assert!(guard.should_suppress("m1", "hash-a", now));
        assert!(!guard.should_suppress("m1", "hash-b", now));
        assert!(!guard.should_suppress("m2", "hash-a", now));

        guard.clear("m1");
        assert!(!guard.should_suppress("m1", "hash-a", now));
    }
}
