use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::sync::engine::Trigger;

/// 同步方向（流水展示用）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LogDirection {
    Upload,
    Download,
}

/// 单条流水的结果。Changed 的 added/removed 为 None 表示有变化但内容超限未统计。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LogOutcome {
    Changed { added: Option<u32>, removed: Option<u32> },
    NoChange,
    Conflict,
    Error { code: String },
}

/// 一条同步操作流水（重启后可查；label 为显示名快照，映射删除后仍可读）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncLogEntry {
    pub ts: DateTime<Utc>,
    pub mapping_id: String,
    pub label: String,
    pub trigger: Trigger,
    pub direction: Option<LogDirection>,
    pub outcome: LogOutcome,
}

/// 流水上限：超出裁掉最旧（与 jsonl 文件裁剪同步）
pub const MAX_LOG_ENTRIES: usize = 2000;

/// 同步操作流水存储：jsonl 追加 + 超上限重写裁剪（仿 snapshot/history 模式）
pub struct SyncLogStore {
    path: PathBuf,
    entries: Mutex<Vec<SyncLogEntry>>,
}

impl SyncLogStore {
    /// 读取既有 jsonl（坏行跳过并 warn 计数；文件不存在视为空）
    pub fn load_from(path: PathBuf) -> Self {
        let mut entries = Vec::new();
        let mut bad = 0u32;
        if let Ok(text) = fs::read_to_string(&path) {
            for line in text.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<SyncLogEntry>(line) {
                    Ok(e) => entries.push(e),
                    Err(_) => bad += 1,
                }
            }
        }
        if bad > 0 {
            tracing::warn!("同步流水日志跳过 {bad} 行损坏记录（{}）", path.display());
        }
        Self {
            path,
            entries: Mutex::new(entries),
        }
    }

    /// 追加一条；超上限裁掉最旧（内存与文件同步，重写走临时文件原子替换）
    pub fn append(&self, entry: SyncLogEntry) -> Result<(), String> {
        let mut entries = self.entries.lock().expect("synclog lock poisoned");
        let overflow = entries.len() + 1 > MAX_LOG_ENTRIES;
        if overflow {
            entries.push(entry.clone());
            let keep_from = entries.len() - MAX_LOG_ENTRIES;
            let drained: Vec<SyncLogEntry> = entries.split_off(keep_from);
            let lines: Vec<String> = drained
                .iter()
                .map(serde_json::to_string)
                .collect::<Result<_, _>>()
                .map_err(|e| format!("序列化流水失败: {e}"))?;
            let tmp = self.path.with_extension("jsonl.tmp");
            fs::write(&tmp, lines.join("\n") + "\n").map_err(|e| format!("重写流水日志失败: {e}"))?;
            fs::rename(&tmp, &self.path).map_err(|e| format!("替换流水日志失败: {e}"))?;
            *entries = drained;
            return Ok(());
        }
        entries.push(entry.clone());
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let line = serde_json::to_string(&entry).map_err(|e| format!("序列化流水失败: {e}"))?;
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("打开流水日志失败: {e}"))?;
        writeln!(f, "{line}").map_err(|e| format!("写入流水日志失败: {e}"))
    }

    /// 尾部记录，新在前
    pub fn tail(&self, limit: Option<usize>) -> Vec<SyncLogEntry> {
        let entries = self.entries.lock().expect("synclog lock poisoned");
        let iter = entries.iter().rev();
        match limit {
            Some(n) => iter.take(n).cloned().collect(),
            None => iter.cloned().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(n: u64) -> SyncLogEntry {
        SyncLogEntry {
            ts: Utc::now() + chrono::Duration::milliseconds(n as i64),
            mapping_id: format!("m{n}"),
            label: format!("映射{n}"),
            trigger: Trigger::Poll,
            direction: Some(LogDirection::Upload),
            outcome: LogOutcome::Changed { added: Some(n as u32), removed: Some(1) },
        }
    }

    #[test]
    fn append_and_tail_roundtrip_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let store = SyncLogStore::load_from(dir.path().join("synclog.jsonl"));
        for n in 0..3 {
            store.append(entry(n)).unwrap();
        }
        let all = store.tail(None);
        assert_eq!(
            all.iter().map(|e| e.mapping_id.as_str()).collect::<Vec<_>>(),
            vec!["m2", "m1", "m0"],
            "tail 新在前"
        );
        assert_eq!(store.tail(Some(2)).len(), 2);
        // 落盘可重新加载
        let reloaded = SyncLogStore::load_from(dir.path().join("synclog.jsonl"));
        assert_eq!(reloaded.tail(None).len(), 3);
    }

    #[test]
    fn prunes_oldest_beyond_cap() {
        let dir = tempfile::tempdir().unwrap();
        let store = SyncLogStore::load_from(dir.path().join("synclog.jsonl"));
        for n in 0..=MAX_LOG_ENTRIES as u64 {
            store.append(entry(n)).unwrap();
        }
        let all = store.tail(None);
        assert_eq!(all.len(), MAX_LOG_ENTRIES);
        assert!(!all.iter().any(|e| e.mapping_id == "m0"), "最旧被裁");
        assert!(all.iter().any(|e| e.mapping_id == "m2000"), "最新保留");
        // 重载后仍为裁剪后的集合
        let reloaded = SyncLogStore::load_from(dir.path().join("synclog.jsonl"));
        assert_eq!(reloaded.tail(None).len(), MAX_LOG_ENTRIES);
    }

    #[test]
    fn corrupt_lines_skipped_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synclog.jsonl");
        let good = serde_json::to_string(&entry(0)).unwrap();
        fs::write(&path, format!("{good}\n{{ not json\n\n{good}\n")).unwrap();
        let store = SyncLogStore::load_from(path);
        let all = store.tail(None);
        assert_eq!(all.len(), 2, "坏行与空行跳过，两条完好记录保留");
    }

    #[test]
    fn outcome_serde_shape() {
        let v = serde_json::to_value(LogOutcome::Changed { added: Some(3), removed: None }).unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "kind": "changed", "added": 3, "removed": null })
        );
        let e = serde_json::to_value(LogOutcome::Error { code: "net".into() }).unwrap();
        assert_eq!(e, serde_json::json!({ "kind": "error", "code": "net" }));
        // Trigger 序列化为 camelCase（前端契约）
        assert_eq!(
            serde_json::to_value(Trigger::ManualUpload).unwrap(),
            serde_json::json!("manualUpload")
        );
    }
}
