use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::Path;

use chrono::{DateTime, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};

use super::snapshot::Side;

/// 归档原因（HistoryEntry 元数据）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HistoryReason {
    /// 远端覆盖本地前归档本地旧内容
    Download,
    /// 本地覆盖远端时归档远端旧内容
    Upload,
    /// 回滚执行时归档当前本地内容
    Rollback,
    /// V0.1/V0.2 遗留的裸归档文件（仅合并结果中出现，不落盘）
    Legacy,
}

/// 一条历史归档记录（jsonl 元数据行；camelCase 供前端消费）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub ts: DateTime<Utc>,
    pub side: Side,
    pub reason: HistoryReason,
    pub file: String,
    pub size_bytes: u64,
}

/// 每映射的 jsonl 元数据文件名
const JSONL_FILE: &str = "history.jsonl";

/// 归档文件名格式校验：`<17位数字>-(local|remote).<非空扩展名>`。
/// 其余（含路径分隔符、`..`）一律拒绝，防路径穿越。
pub fn is_archive_name(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    if ext.is_empty() || ext.contains('/') || ext.contains('\\') {
        return false;
    }
    let Some((digits, side)) = stem.split_once('-') else {
        return false;
    };
    digits.len() == 17
        && digits.bytes().all(|b| b.is_ascii_digit())
        && Side::parse(side).is_some()
}

/// 追加一条元数据行（append 天然并发安全）
pub fn append_entry(mapping_dir: &Path, entry: &HistoryEntry) -> std::io::Result<()> {
    fs::create_dir_all(mapping_dir)?;
    let line =
        serde_json::to_string(entry).map_err(|e| std::io::Error::other(e.to_string()))?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(mapping_dir.join(JSONL_FILE))?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")
}

/// 读归档文件内容。文件名不合法（防路径穿越）→ InvalidInput（错误信息即错误码），
/// 文件不存在 → NotFound。
pub fn read_archive(mapping_dir: &Path, file_name: &str) -> std::io::Result<String> {
    if !is_archive_name(file_name) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "HISTORY_FILE_INVALID",
        ));
    }
    fs::read_to_string(mapping_dir.join(file_name))
}

/// 历史列表：jsonl 行 + 目录中不在 jsonl 里的裸归档文件（legacy），按 ts 倒序。
/// 坏行/坏文件名跳过，不报错。
pub fn list_entries(mapping_dir: &Path) -> Vec<HistoryEntry> {
    let mut entries: Vec<HistoryEntry> = Vec::new();
    let mut files_in_jsonl: HashSet<String> = HashSet::new();
    if let Ok(text) = fs::read_to_string(mapping_dir.join(JSONL_FILE)) {
        for line in text.lines() {
            if let Ok(e) = serde_json::from_str::<HistoryEntry>(line) {
                files_in_jsonl.insert(e.file.clone());
                entries.push(e);
            }
        }
    }
    if let Ok(rd) = fs::read_dir(mapping_dir) {
        for ent in rd.flatten() {
            let name = ent.file_name().to_string_lossy().to_string();
            if !is_archive_name(&name) || files_in_jsonl.contains(&name) {
                continue;
            }
            if let Some(e) = legacy_entry(mapping_dir, &name) {
                entries.push(e);
            }
        }
    }
    entries.sort_by_key(|e| std::cmp::Reverse(e.ts));
    entries
}

/// 按文件名时间戳升序删除最旧归档（含裸文件），超限才动；jsonl 原子重写为保留行。
pub fn prune(mapping_dir: &Path, keep: usize) -> std::io::Result<()> {
    if !mapping_dir.is_dir() {
        return Ok(());
    }
    let mut files: Vec<String> = Vec::new();
    for ent in fs::read_dir(mapping_dir)?.flatten() {
        let name = ent.file_name().to_string_lossy().to_string();
        if is_archive_name(&name) {
            files.push(name);
        }
    }
    files.sort();
    let del = files.len().saturating_sub(keep);
    if del > 0 {
        for name in &files[..del] {
            fs::remove_file(mapping_dir.join(name))?;
        }
    }
    let survivors: HashSet<&str> = files[del..].iter().map(String::as_str).collect();

    let mut kept = String::new();
    if let Ok(text) = fs::read_to_string(mapping_dir.join(JSONL_FILE)) {
        for line in text.lines() {
            if let Ok(e) = serde_json::from_str::<HistoryEntry>(line)
                && survivors.contains(e.file.as_str())
            {
                kept.push_str(line);
                kept.push('\n');
            }
        }
    }
    let tmp = mapping_dir.join("history.jsonl.tmp");
    fs::write(&tmp, kept)?;
    fs::rename(tmp, mapping_dir.join(JSONL_FILE))
}

/// 裸归档文件名 → 元数据（ts/side 取自文件名，size 取自磁盘）
fn legacy_entry(mapping_dir: &Path, name: &str) -> Option<HistoryEntry> {
    let (stem, _ext) = name.rsplit_once('.')?;
    let (digits, side) = stem.split_once('-')?;
    let naive = NaiveDateTime::parse_from_str(digits, "%Y%m%d%H%M%S%3f").ok()?;
    let size = fs::metadata(mapping_dir.join(name)).ok()?.len();
    Some(HistoryEntry {
        ts: naive.and_utc(),
        side: Side::parse(side)?,
        reason: HistoryReason::Legacy,
        file: name.to_string(),
        size_bytes: size,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::fs;
    use std::io::Write;

    use chrono::TimeZone;

    use super::*;

    fn entry(ts_millis: i64, side: Side, reason: HistoryReason, file: &str, size: u64) -> HistoryEntry {
        HistoryEntry {
            ts: Utc.timestamp_millis_opt(ts_millis).unwrap(),
            side,
            reason,
            file: file.to_string(),
            size_bytes: size,
        }
    }

    #[test]
    fn append_list_roundtrip_and_serde_values() {
        let dir = tempfile::tempdir().unwrap();
        let e = entry(
            1767227445123,
            Side::Local,
            HistoryReason::Download,
            "20260101000000001-local.md",
            123,
        );
        append_entry(dir.path(), &e).unwrap();

        let listed = list_entries(dir.path());
        assert_eq!(listed, vec![e.clone()]);

        // 序列化契约：side/reason 小写、字段 camelCase
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains(r#""side":"local""#), "json={json}");
        assert!(json.contains(r#""reason":"download""#), "json={json}");
        assert!(json.contains(r#""sizeBytes":123"#), "json={json}");
    }

    #[test]
    fn bare_file_merged_as_legacy_and_state_json_excluded() {
        let dir = tempfile::tempdir().unwrap();
        // 裸归档（无 jsonl 行）
        fs::write(dir.path().join("20260101000000000-local.md"), "legacy content").unwrap();
        // jsonl 行 + 对应归档文件
        let e = entry(
            1767227445001,
            Side::Remote,
            HistoryReason::Upload,
            "20260101000000001-remote.txt",
            10,
        );
        fs::write(dir.path().join("20260101000000001-remote.txt"), "0123456789").unwrap();
        append_entry(dir.path(), &e).unwrap();
        // state.json 不属于历史
        fs::write(dir.path().join("state.json"), "{}").unwrap();

        let listed = list_entries(dir.path());
        assert_eq!(listed.len(), 2, "listed={listed:?}");
        // ts 倒序：jsonl 行（.001）在前
        assert_eq!(listed[0], e);
        let legacy = &listed[1];
        assert_eq!(legacy.reason, HistoryReason::Legacy);
        assert_eq!(legacy.side, Side::Local);
        assert_eq!(legacy.file, "20260101000000000-local.md");
        assert_eq!(legacy.size_bytes, "legacy content".len() as u64);
    }

    #[test]
    fn prune_removes_oldest_and_rewrites_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        // 105 条：前 60 条带 jsonl 行，后 45 条为裸文件
        for i in 0..105u64 {
            let ts_num = 20260101000000000u64 + i;
            let name = format!("{ts_num}-local.md");
            fs::write(dir.path().join(&name), format!("c{i}")).unwrap();
            if i < 60 {
                let e = entry(
                    1767227445000 + i as i64,
                    Side::Local,
                    HistoryReason::Download,
                    &name,
                    2,
                );
                append_entry(dir.path(), &e).unwrap();
            }
        }

        prune(dir.path(), 100).unwrap();

        let remaining: HashSet<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .filter(|n| is_archive_name(n))
            .collect();
        assert_eq!(remaining.len(), 100, "remaining={remaining:?}");
        // 最旧 5 个被删（前 60 条里最老的 5 个 jsonl 行条目）
        for i in 0..5u64 {
            assert!(!remaining.contains(&format!("{}-local.md", 20260101000000000u64 + i)));
        }
        // jsonl 重写只含保留行（60 - 5 = 55）
        let jsonl = fs::read_to_string(dir.path().join("history.jsonl")).unwrap();
        let rows = jsonl.lines().filter(|l| !l.trim().is_empty()).count();
        assert_eq!(rows, 55, "jsonl rows={rows}");
    }

    #[test]
    fn is_archive_name_table() {
        assert!(is_archive_name("20260101000000000-local.md"));
        assert!(is_archive_name("20260101000000000-remote.txt"));
        for bad in [
            "..\\config.json",
            "a/b-local.md",
            "20260101000000000-local",
            "xx-local.md",
            "20260101000000000-local.md.exe/../x",
        ] {
            assert!(!is_archive_name(bad), "should reject: {bad}");
        }
    }

    #[test]
    fn corrupt_jsonl_line_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let e = entry(
            1767227445123,
            Side::Remote,
            HistoryReason::Rollback,
            "20260101000000001-remote.md",
            5,
        );
        append_entry(dir.path(), &e).unwrap();
        // 进程中途被杀留下半行 JSON
        use std::io::Write;
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(dir.path().join("history.jsonl"))
            .unwrap();
        f.write_all(br#"{"ts":"2026"#).unwrap();

        let listed = list_entries(dir.path());
        assert_eq!(listed, vec![e]);
    }
}
