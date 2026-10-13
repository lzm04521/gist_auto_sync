use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::ConfigStore;
use crate::model::{Account, Mapping};
use crate::provider::{build_provider, GistProvider, ProviderError};
use crate::secrets;
use crate::sync::compare::{decide, normalize, sha256_hex, CompareInput, Decision};
use crate::sync::snapshot::{SelfWriteGuard, Side, SnapshotState, SnapshotStore};

/// 触发源
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// watcher 防抖后的本地稳定事件
    LocalSettled,
    /// 轮询
    Poll,
    /// 手动"立即上传"（同时充当冲突解决-用本地覆盖远端）
    ManualUpload,
    /// 手动"立即下载"（同时充当冲突解决-用远端覆盖本地）
    ManualDownload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MappingStatus {
    Idle,
    Uploading,
    Downloading,
    Conflict,
    Error,
    Missing,
    Paused,
}

/// 推送给 UI 的状态事件（T12 转成 Tauri event）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusEvent {
    pub mapping_id: String,
    pub status: MappingStatus,
    pub message: Option<String>,
    pub last_synced_at: Option<DateTime<Utc>>,
}

pub trait StatusSink: Send + Sync {
    fn emit(&self, event: StatusEvent);
}

/// 网络失败重试退避：1s · 2^(n-1)，封顶 5 分钟（spec §6.6）
pub fn backoff_delay(attempt: u32) -> Duration {
    let secs = 1u64
        .checked_shl(attempt.saturating_sub(1))
        .unwrap_or(u64::MAX);
    Duration::from_secs(secs.min(300))
}

/// 账号 → provider 缓存（token 只在构建时加载一次；账号更新后 invalidate）
#[derive(Default)]
pub struct ProviderCache {
    inner: Mutex<HashMap<String, Arc<dyn GistProvider>>>,
}

impl ProviderCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// 测试注入 / 手动注册
    pub fn insert(&self, account_id: &str, provider: Arc<dyn GistProvider>) {
        self.inner
            .lock()
            .expect("provider cache lock poisoned")
            .insert(account_id.to_string(), provider);
    }

    pub fn invalidate(&self, account_id: &str) {
        self.inner
            .lock()
            .expect("provider cache lock poisoned")
            .remove(account_id);
    }

    pub async fn get_or_build(&self, account: &Account) -> Result<Arc<dyn GistProvider>, ProviderError> {
        if let Some(p) = self
            .inner
            .lock()
            .expect("provider cache lock poisoned")
            .get(&account.id)
        {
            return Ok(p.clone());
        }
        let token = secrets::load_token(&account.token_ref)
            .map_err(|_| ProviderError::Auth)?;
        let provider: Arc<dyn GistProvider> = Arc::from(build_provider(account, &token));
        self.insert(&account.id, provider.clone());
        Ok(provider)
    }
}

/// 引擎上下文：一次构造、全应用共享
pub struct SyncCtx {
    pub config: Arc<ConfigStore>,
    pub snapshots: Arc<SnapshotStore>,
    pub providers: ProviderCache,
    pub guard: SelfWriteGuard,
    pub sink: Arc<dyn StatusSink>,
}

/// 单次同步循环（spec §6.1/6.2/6.5）。返回最终状态；过程经 sink 推送。
pub async fn run_sync_cycle(ctx: &SyncCtx, mapping: &Mapping, trigger: Trigger) -> MappingStatus {
    // ── 本地读取（非 UTF-8 → Error，映射创建时已校验过，此处兜底）
    let (local_raw, local_mtime) = match std::fs::read_to_string(&mapping.local_path) {
        Ok(text) => {
            let mtime = std::fs::metadata(&mapping.local_path)
                .ok()
                .and_then(|m| m.modified().ok());
            (Some(text), mtime)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (None, None),
        Err(e) => {
            return emit_status(ctx, mapping, MappingStatus::Error, Some(format!("读取本地文件失败: {e}")), false);
        }
    };
    let local_hash = local_raw.as_ref().map(|t| sha256_hex(&normalize(t)));

    // ── 自写抑制：下载写盘触发的 watcher 事件直接忽略（spec §6.1）
    if trigger == Trigger::LocalSettled
        && let (Some(h), Some(m)) = (&local_hash, local_mtime)
        && ctx.guard.should_suppress(&mapping.id, h, m)
    {
        return MappingStatus::Idle;
    }

    // ── provider 解析
    let account = ctx
        .config
        .read()
        .accounts
        .into_iter()
        .find(|a| a.id == mapping.account_id);
    let Some(account) = account else {
        return emit_status(ctx, mapping, MappingStatus::Error, Some("账号不存在".into()), false);
    };
    let provider = match ctx.providers.get_or_build(&account).await {
        Ok(p) => p,
        Err(e) => {
            return emit_status(ctx, mapping, MappingStatus::Error, Some(e.to_string()), false)
        }
    };

    // ── 远端读取
    let detail = match provider.get_gist(&mapping.gist_id).await {
        Ok(d) => d,
        Err(e) => {
            return emit_status(ctx, mapping, MappingStatus::Error, Some(e.to_string()), false)
        }
    };
    let remote_file = detail
        .files
        .iter()
        .find(|f| f.filename == mapping.gist_filename);
    let remote_raw = remote_file.and_then(|f| f.content.clone());
    let remote_hash = remote_raw.as_ref().map(|t| sha256_hex(&normalize(t)));

    // ── 三方比较
    let snapshot = ctx.snapshots.load(&mapping.id);
    let input = CompareInput {
        local_exists: local_raw.is_some(),
        local_hash: local_hash.clone(),
        snapshot_hash: snapshot
            .as_ref()
            .map(|s| s.last_synced_hash.clone()),
        remote_file_exists: remote_file.is_some(),
        remote_hash: remote_hash.clone(),
    };

    // ── 手动语义改写：显式覆盖意图优先于自动决策（spec：Manual 兼作冲突解决）
    let decision = match trigger {
        Trigger::ManualUpload => {
            let Some(local) = &local_raw else {
                return emit_status(ctx, mapping, MappingStatus::Error, Some("本地文件不存在".into()), false);
            };
            let local_changed = snapshot
                .as_ref()
                .map(|s| s.last_synced_hash != local_hash.clone().unwrap())
                .unwrap_or(true);
            let _ = local;
            if !local_changed {
                return emit_status(ctx, mapping, MappingStatus::Idle, Some("本地无未同步修改".into()), false);
            }
            Decision::Upload
        }
        Trigger::ManualDownload => {
            let Some(remote) = &remote_raw else {
                return emit_status(ctx, mapping, MappingStatus::Error, Some("远端不存在该文件".into()), false);
            };
            let remote_changed = snapshot
                .as_ref()
                .map(|s| s.last_synced_hash != remote_hash.clone().unwrap())
                .unwrap_or(true);
            let _ = remote;
            if !remote_changed {
                return emit_status(ctx, mapping, MappingStatus::Idle, Some("远端无变更".into()), false);
            }
            Decision::Download
        }
        _ => decide(&input),
    };

    let is_auto = matches!(trigger, Trigger::LocalSettled | Trigger::Poll);

    match decision {
        Decision::Noop => MappingStatus::Idle,
        Decision::SeedSnapshot => {
            let hash = input.local_hash.clone().unwrap_or_default();
            let state = SnapshotState {
                last_synced_hash: hash,
                last_synced_at: Utc::now(),
                last_direction: None,
                remote_updated_at: Some(detail.updated_at),
            };
            if let Err(e) = ctx.snapshots.save(&mapping.id, &state) {
                return emit_status(ctx, mapping, MappingStatus::Error, Some(format!("保存快照失败: {e}")), false);
            }
            emit_status(ctx, mapping, MappingStatus::Idle, Some("已建立同步基准".into()), true)
        }
        Decision::MissingLocal => {
            emit_status(ctx, mapping, MappingStatus::Missing, Some("本地文件不存在或已删除".into()), false)
        }
        Decision::MissingRemote => {
            emit_status(ctx, mapping, MappingStatus::Missing, Some("远端 gist 中不存在该文件".into()), false)
        }
        Decision::Conflict => {
            emit_status(ctx, mapping, MappingStatus::Conflict, Some("本地与远端均有未同步修改，请人工选择保留哪边".into()), false)
        }
        Decision::Upload => {
            if !mapping.upload_enabled && is_auto {
                return emit_status(ctx, mapping, MappingStatus::Idle, Some("上传开关已关闭，跳过上传".into()), false);
            }
            emit_status(ctx, mapping, MappingStatus::Uploading, None, false);
            // 归档旧远端（远端文件不存在时无可归档）
            if let Some(remote) = &remote_raw {
                let _ = ctx
                    .snapshots
                    .archive(&mapping.id, Side::Remote, remote, &ext_of(&mapping.gist_filename));
            }
            // 上传规范化内容（CRLF→LF），快照以回读为准
            let content = normalize(local_raw.as_deref().expect("Upload 分支必有本地内容"));
            match provider
                .update_file(&mapping.gist_id, &mapping.gist_filename, &content)
                .await
            {
                Ok(new_detail) => {
                    let back = new_detail
                        .files
                        .iter()
                        .find(|f| f.filename == mapping.gist_filename)
                        .and_then(|f| f.content.clone())
                        .unwrap_or(content);
                    let state = SnapshotState {
                        last_synced_hash: sha256_hex(&normalize(&back)),
                        last_synced_at: Utc::now(),
                        last_direction: Some(crate::sync::snapshot::SyncDirection::Upload),
                        remote_updated_at: Some(new_detail.updated_at),
                    };
                    if let Err(e) = ctx.snapshots.save(&mapping.id, &state) {
                        return emit_status(ctx, mapping, MappingStatus::Error, Some(format!("保存快照失败: {e}")), false);
                    }
                    emit_status(ctx, mapping, MappingStatus::Idle, None, true)
                }
                Err(e) => emit_status(ctx, mapping, MappingStatus::Error, Some(e.to_string()), false),
            }
        }
        Decision::Download => {
            if !mapping.download_enabled && is_auto {
                return emit_status(ctx, mapping, MappingStatus::Idle, Some("下载开关已关闭，跳过下载".into()), false);
            }
            emit_status(ctx, mapping, MappingStatus::Downloading, None, false);
            // 归档旧本地
            if let Some(local) = &local_raw {
                let _ = ctx
                    .snapshots
                    .archive(&mapping.id, Side::Local, local, &ext_of(&mapping.local_path));
            }
            // 写盘保留远端原始字节（spec §6.5）
            let raw = remote_raw.clone().expect("Download 分支必有远端内容");
            if let Err(e) = std::fs::write(&mapping.local_path, raw.as_bytes()) {
                return emit_status(ctx, mapping, MappingStatus::Error, Some(format!("写入本地文件失败: {e}")), false);
            }
            let mtime = std::fs::metadata(&mapping.local_path)
                .ok()
                .and_then(|m| m.modified().ok());
            let hash = sha256_hex(&normalize(&raw));
            if let Some(m) = mtime {
                ctx.guard.register(&mapping.id, &hash, m);
            }
            let state = SnapshotState {
                last_synced_hash: hash,
                last_synced_at: Utc::now(),
                last_direction: Some(crate::sync::snapshot::SyncDirection::Download),
                remote_updated_at: Some(detail.updated_at),
            };
            if let Err(e) = ctx.snapshots.save(&mapping.id, &state) {
                return emit_status(ctx, mapping, MappingStatus::Error, Some(format!("保存快照失败: {e}")), false);
            }
            emit_status(ctx, mapping, MappingStatus::Idle, None, true)
        }
    }
}

fn ext_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_else(|| "txt".to_string())
}

fn emit_status(ctx: &SyncCtx, mapping: &Mapping, status: MappingStatus, message: Option<String>, synced: bool) -> MappingStatus {
    ctx.sink.emit(StatusEvent {
        mapping_id: mapping.id.clone(),
        status,
        message,
        last_synced_at: synced.then(Utc::now),
    });
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{GistDetail, GistSummary, UserInfo};
    use std::fs;

    /// 测试用可编程 provider
    struct FakeProvider {
        /// (gist_id, filename, content)
        files: Mutex<Vec<(String, String, String)>>,
        fail_next: Mutex<Option<ProviderError>>,
        calls: Mutex<Vec<&'static str>>,
    }

    impl FakeProvider {
        fn new() -> Self {
            Self {
                files: Mutex::new(vec![]),
                fail_next: Mutex::new(None),
                calls: Mutex::new(vec![]),
            }
        }

        fn set_file(&self, gist_id: &str, filename: &str, content: &str) {
            self.files.lock().unwrap().retain(|(g, f, _)| !(g == gist_id && f == filename));
            self.files
                .lock()
                .unwrap()
                .push((gist_id.into(), filename.into(), content.into()));
        }

        fn remove_file(&self, gist_id: &str, filename: &str) {
            self.files
                .lock()
                .unwrap()
                .retain(|(g, f, _)| !(g == gist_id && f == filename));
        }

        fn calls(&self) -> usize {
            self.calls.lock().unwrap().len()
        }

        fn update_calls(&self) -> usize {
            self.calls.lock().unwrap().iter().filter(|c| **c == "update").count()
        }
    }

    #[async_trait::async_trait]
    impl GistProvider for FakeProvider {
        async fn verify_token(&self) -> Result<UserInfo, ProviderError> {
            Ok(UserInfo { login: "fake".into(), name: None })
        }
        async fn list_gists(&self, _since: Option<DateTime<Utc>>) -> Result<Vec<GistSummary>, ProviderError> {
            Ok(vec![])
        }
        async fn get_gist(&self, gist_id: &str) -> Result<GistDetail, ProviderError> {
            self.calls.lock().unwrap().push("get");
            if let Some(e) = self.fail_next.lock().unwrap().take() {
                return Err(e);
            }
            let files = self
                .files
                .lock()
                .unwrap()
                .iter()
                .filter(|(g, _, _)| g == gist_id)
                .map(|(_, f, c)| crate::provider::GistFile {
                    filename: f.clone(),
                    raw_url: None,
                    content: Some(c.clone()),
                })
                .collect();
            Ok(GistDetail {
                id: gist_id.to_string(),
                description: None,
                updated_at: Utc::now(),
                files,
            })
        }
        async fn update_file(&self, gist_id: &str, filename: &str, content: &str) -> Result<GistDetail, ProviderError> {
            self.calls.lock().unwrap().push("update");
            if let Some(e) = self.fail_next.lock().unwrap().take() {
                return Err(e);
            }
            self.set_file(gist_id, filename, content);
            self.get_gist(gist_id).await
        }
    }

    #[derive(Default)]
    struct VecSink(Mutex<Vec<StatusEvent>>);

    impl StatusSink for VecSink {
        fn emit(&self, event: StatusEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    fn sha(s: &str) -> String {
        sha256_hex(&normalize(s))
    }

    struct Fixture {
        ctx: SyncCtx,
        fake: Arc<FakeProvider>,
        mapping: Mapping,
        local_path: std::path::PathBuf,
        snap_root: std::path::PathBuf,
        #[allow(dead_code)]
        dir: tempfile::TempDir,
    }

    fn fixture(local_content: &str, remote_content: &str, snapshot_content: Option<&str>) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let local_path = dir.path().join("notes.md");
        fs::write(&local_path, local_content).unwrap();

        let config = ConfigStore::load_from(dir.path().join("config.json")).unwrap();
        config
            .update(|c| {
                c.accounts.push(Account {
                    id: "acc1".into(),
                    name: "测试".into(),
                    provider: crate::model::ProviderKind::Github,
                    base_url: "https://api.github.com".into(),
                    token_ref: "unused".into(),
                });
            })
            .unwrap();

        let snap_root = dir.path().join("snapshots");
        let snapshots = Arc::new(SnapshotStore::new(snap_root.clone()));
        let fake = Arc::new(FakeProvider::new());
        fake.set_file("gist1", "notes.md", remote_content);
        let providers = ProviderCache::new();
        providers.insert("acc1", fake.clone());
        let ctx = SyncCtx {
            config: Arc::new(config),
            snapshots,
            providers,
            guard: SelfWriteGuard::new(),
            sink: Arc::new(VecSink::default()),
        };
        let mapping = Mapping {
            id: "m1".into(),
            local_path: local_path.to_string_lossy().to_string(),
            account_id: "acc1".into(),
            gist_id: "gist1".into(),
            gist_filename: "notes.md".into(),
            upload_enabled: true,
            download_enabled: true,
            enabled: true,
        };
        if let Some(s) = snapshot_content {
            ctx.snapshots
                .save(
                    "m1",
                    &SnapshotState {
                        last_synced_hash: sha(s),
                        last_synced_at: Utc::now(),
                        last_direction: Some(crate::sync::snapshot::SyncDirection::Upload),
                        remote_updated_at: None,
                    },
                )
                .unwrap();
        }
        Fixture { ctx, fake, mapping, local_path, snap_root, dir }
    }

    fn history_files(root: &Path, side: &str) -> Vec<String> {
        let dir = root.join("m1");
        let mut out = vec![];
        if let Ok(rd) = fs::read_dir(&dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.contains(&format!("-{side}.")) {
                    out.push(fs::read_to_string(e.path()).unwrap_or_default());
                }
            }
        }
        out
    }

    #[tokio::test]
    async fn upload_patches_updates_snapshot_and_archives_remote() {
        let f = fixture("new local\r\n", "old remote", Some("old remote"));
        let status = run_sync_cycle(&f.ctx, &f.mapping, Trigger::LocalSettled).await;
        assert_eq!(status, MappingStatus::Idle);
        assert_eq!(f.fake.update_calls(), 1);
        // 快照 = 回读内容（规范化后）
        assert_eq!(f.ctx.snapshots.load("m1").unwrap().last_synced_hash, sha("new local\n"));
        // 旧远端内容已归档
        assert!(history_files(&f.snap_root, "remote").contains(&"old remote".to_string()));
    }

    #[tokio::test]
    async fn download_writes_raw_registers_guard_and_archives_local() {
        let f = fixture("old local", "new remote", Some("old local"));
        let status = run_sync_cycle(&f.ctx, &f.mapping, Trigger::Poll).await;
        assert_eq!(status, MappingStatus::Idle);
        // 本地文件 = 远端原始字节
        assert_eq!(fs::read_to_string(&f.local_path).unwrap(), "new remote");
        // 自写抑制已登记
        let mtime = fs::metadata(&f.local_path).unwrap().modified().unwrap();
        assert!(f.ctx.guard.should_suppress("m1", &sha("new remote"), mtime));
        // 旧本地已归档
        assert!(history_files(&f.snap_root, "local").contains(&"old local".to_string()));
        assert_eq!(f.fake.update_calls(), 0);
    }

    #[tokio::test]
    async fn local_settled_after_download_is_suppressed_no_api_call() {
        let f = fixture("old local", "new remote", Some("old local"));
        run_sync_cycle(&f.ctx, &f.mapping, Trigger::Poll).await;
        let calls = f.fake.calls();
        let status = run_sync_cycle(&f.ctx, &f.mapping, Trigger::LocalSettled).await;
        assert_eq!(status, MappingStatus::Idle);
        assert_eq!(f.fake.calls(), calls, "自写抑制：不应产生任何 API 调用");
    }

    #[tokio::test]
    async fn both_changed_yields_conflict_without_writes() {
        let f = fixture("local v2", "remote v2", Some("base"));
        let status = run_sync_cycle(&f.ctx, &f.mapping, Trigger::LocalSettled).await;
        assert_eq!(status, MappingStatus::Conflict);
        assert_eq!(f.fake.update_calls(), 0);
        // 本地与远端都未被改写
        assert_eq!(fs::read_to_string(&f.local_path).unwrap(), "local v2");
    }

    #[tokio::test]
    async fn remote_file_missing_yields_missing_without_writes() {
        let f = fixture("local", "irrelevant", Some("base"));
        f.fake.remove_file("gist1", "notes.md");
        let status = run_sync_cycle(&f.ctx, &f.mapping, Trigger::Poll).await;
        assert_eq!(status, MappingStatus::Missing);
        assert_eq!(f.fake.update_calls(), 0);
    }

    #[tokio::test]
    async fn network_error_yields_error_status_and_backoff_caps() {
        let f = fixture("local", "remote", Some("base"));
        *f.fake.fail_next.lock().unwrap() = Some(ProviderError::Network("dns boom".into()));
        let status = run_sync_cycle(&f.ctx, &f.mapping, Trigger::Poll).await;
        assert_eq!(status, MappingStatus::Error);
        assert_eq!(backoff_delay(1), Duration::from_secs(1));
        assert_eq!(backoff_delay(9), Duration::from_secs(256));
        assert_eq!(backoff_delay(20), Duration::from_secs(300));
        assert_eq!(backoff_delay(1000), Duration::from_secs(300));
    }

    #[tokio::test]
    async fn manual_upload_overrides_conflict() {
        let f = fixture("local v2", "remote v2", Some("base"));
        let status = run_sync_cycle(&f.ctx, &f.mapping, Trigger::ManualUpload).await;
        assert_eq!(status, MappingStatus::Idle);
        assert_eq!(f.fake.update_calls(), 1);
        assert_eq!(f.ctx.snapshots.load("m1").unwrap().last_synced_hash, sha("local v2"));
    }

    #[tokio::test]
    async fn manual_download_overrides_conflict() {
        let f = fixture("local v2", "remote v2", Some("base"));
        let status = run_sync_cycle(&f.ctx, &f.mapping, Trigger::ManualDownload).await;
        assert_eq!(status, MappingStatus::Idle);
        assert_eq!(fs::read_to_string(&f.local_path).unwrap(), "remote v2");
    }

    #[tokio::test]
    async fn first_sync_equal_seeds_snapshot_differ_conflicts() {
        let f = fixture("same", "same", None);
        let status = run_sync_cycle(&f.ctx, &f.mapping, Trigger::Poll).await;
        assert_eq!(status, MappingStatus::Idle);
        assert_eq!(f.fake.calls(), 1, "仅 get，无 update");
        assert_eq!(f.ctx.snapshots.load("m1").unwrap().last_synced_hash, sha("same"));

        let f2 = fixture("mine", "theirs", None);
        let status2 = run_sync_cycle(&f2.ctx, &f2.mapping, Trigger::Poll).await;
        assert_eq!(status2, MappingStatus::Conflict);
    }

    #[tokio::test]
    async fn direction_switch_gates_auto_but_not_conflict() {
        // 上传开关关闭：本地改了 → 自动触发不动作
        let mut f = fixture("local v2", "base", Some("base"));
        f.mapping.upload_enabled = false;
        let status = run_sync_cycle(&f.ctx, &f.mapping, Trigger::LocalSettled).await;
        assert_eq!(status, MappingStatus::Idle);
        assert_eq!(f.fake.update_calls(), 0);

        // 但双方都变仍判冲突（方向开关不豁免冲突）
        let mut f2 = fixture("local v2", "remote v2", Some("base"));
        f2.mapping.upload_enabled = false;
        f2.mapping.download_enabled = false;
        let status2 = run_sync_cycle(&f2.ctx, &f2.mapping, Trigger::Poll).await;
        assert_eq!(status2, MappingStatus::Conflict);
    }
}

