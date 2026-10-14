use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
}

impl SnapshotStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
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

    /// 覆盖前归档被覆盖一侧的旧内容：<ts>-{local|remote}.<ext>
    pub fn archive(
        &self,
        mapping_id: &str,
        side: Side,
        content: &str,
        ext: &str,
    ) -> std::io::Result<PathBuf> {
        let dir = self.mapping_dir(mapping_id);
        fs::create_dir_all(&dir)?;
        let ts = Utc::now().format("%Y%m%d%H%M%S%3f");
        let path = dir.join(format!("{ts}-{}.{ext}", side.as_str()));
        fs::write(&path, content)?;
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
    use super::*;

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
            .archive("m1", Side::Local, "旧本地内容", "md")
            .unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.ends_with("-local.md"), "name={name}");
        assert!(name.len() > "-local.md".len() + 15, "name={name}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "旧本地内容");

        let remote = store
            .archive("m1", Side::Remote, "old remote", "txt")
            .unwrap();
        assert!(remote
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with("-remote.txt"));
    }

    #[test]
    fn remove_mapping_deletes_dir() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path().to_path_buf());
        store.archive("m1", Side::Local, "x", "md").unwrap();
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
