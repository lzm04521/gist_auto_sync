use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::model::Mapping;
use crate::sync::engine::{run_sync_cycle, MappingStatus, SyncCtx, Trigger};
use crate::sync::watcher::FileWatcher;

/// 持久化状态（statuses.json），重启后恢复 Conflict/Error/Missing/Paused
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedStatus {
    pub status: MappingStatus,
    pub message: Option<String>,
    pub last_synced_at: Option<DateTime<Utc>>,
}

/// 引擎服务：装配 watcher 防抖、轮询、重试队列与状态持久化
pub struct Engine {
    pub ctx: Arc<SyncCtx>,
    statuses: Mutex<HashMap<String, PersistedStatus>>,
    statuses_path: PathBuf,
    paused: AtomicBool,
    retry_attempts: Mutex<HashMap<String, u32>>,
    last_poll: Mutex<HashMap<String, DateTime<Utc>>>,
    watcher: Mutex<Option<FileWatcher>>,
    settled_tx: mpsc::Sender<PathBuf>,
    settled_rx: Mutex<Option<mpsc::Receiver<PathBuf>>>,
}

impl Engine {
    /// 构造并加载 statuses.json（不存在 → 空）
    pub fn new(ctx: Arc<SyncCtx>, statuses_path: PathBuf) -> std::io::Result<Self> {
        let statuses = std::fs::read_to_string(&statuses_path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let (settled_tx, settled_rx) = mpsc::channel(256);
        Ok(Self {
            ctx,
            statuses: Mutex::new(statuses),
            statuses_path,
            paused: AtomicBool::new(false),
            retry_attempts: Mutex::new(HashMap::new()),
            last_poll: Mutex::new(HashMap::new()),
            watcher: Mutex::new(None),
            settled_tx,
            settled_rx: Mutex::new(Some(settled_rx)),
        })
    }

    /// 启动常驻任务：settled 分发器 + 轮询循环
    pub fn start(self: &Arc<Self>) -> Result<(), String> {
        self.apply_watch_set()?;

        // settled 分发器：接管 settled_rx（重建 watcher 只换生产端）
        let rx = self
            .settled_rx
            .lock()
            .expect("settled_rx lock poisoned")
            .take();
        if let Some(mut rx) = rx {
            let engine = self.clone();
            tokio::spawn(async move {
                while let Some(path) = rx.recv().await {
                    engine.dispatch_settled(path).await;
                }
            });
        }

        // 轮询循环
        let engine = self.clone();
        tokio::spawn(engine.poll_loop());
        Ok(())
    }

    /// 按"轮询间隔重读设置"的循环；每轮对 download_enabled 且 enabled 的映射跑增量轮询
    async fn poll_loop(self: Arc<Self>) {
        loop {
            let mins = self
                .ctx
                .config
                .read()
                .settings
                .poll_interval_minutes
                .clamp(1, 1440);
            tokio::time::sleep(Duration::from_secs(mins * 60)).await;
            if self.is_global_paused() {
                continue;
            }
            self.poll_round().await;
        }
    }

    /// 一轮增量轮询：按账号 list_gists(since=上次轮询时间) 粗筛，命中才跑 cycle
    pub async fn poll_round(self: &Arc<Self>) {
        let cfg = self.ctx.config.read();
        let accounts = cfg.accounts.clone();
        let mappings: Vec<Mapping> = cfg
            .mappings
            .iter()
            .filter(|m| m.enabled && m.download_enabled)
            .cloned()
            .collect();
        drop(cfg);

        // 粗筛：每账号拉一次列表，收集有变更的 (account_id, gist_id)
        let mut changed: HashSet<(String, String)> = HashSet::new();
        for account in &accounts {
            let has_mappings = mappings.iter().any(|m| m.account_id == account.id);
            if !has_mappings {
                continue;
            }
            let Ok(provider) = self.ctx.providers.get_or_build(account).await else {
                continue;
            };
            let since = self
                .last_poll
                .lock()
                .expect("last_poll lock poisoned")
                .get(&account.id)
                .cloned();
            if let Ok(list) = provider.list_gists(since).await {
                for g in list {
                    changed.insert((account.id.clone(), g.id));
                }
            }
            self.last_poll
                .lock()
                .expect("last_poll lock poisoned")
                .insert(account.id.clone(), Utc::now());
        }

        for m in mappings {
            // 无快照的映射（新配置）也要对账
            let no_snapshot = self.ctx.snapshots.load(&m.id).is_none();
            if no_snapshot
                || changed.contains(&(m.account_id.clone(), m.gist_id.clone()))
            {
                self.run_mapping_tracked(&m, Trigger::Poll).await;
            }
        }
    }

    /// watcher settled 路径分发：映射到具体 mapping 执行 LocalSettled cycle（全局暂停时跳过）
    pub async fn dispatch_settled(self: &Arc<Self>, path: PathBuf) {
        if self.is_global_paused() {
            return;
        }
        let cfg = self.ctx.config.read();
        let target = cfg
            .mappings
            .iter()
            .filter(|m| m.enabled)
            .find(|m| path_eq(&PathBuf::from(&m.local_path), &path))
            .cloned();
        drop(cfg);
        if let Some(m) = target {
            self.run_mapping_tracked(&m, Trigger::LocalSettled).await;
        }
    }

    /// 启动对账：对所有 enabled 且非 Conflict 的映射跑一次 Poll（应用启动/重启后收敛状态）
    pub async fn startup_reconcile(self: &Arc<Self>) {
        let cfg = self.ctx.config.read();
        let targets: Vec<Mapping> = cfg
            .mappings
            .iter()
            .filter(|m| m.enabled && m.download_enabled)
            .cloned()
            .collect();
        drop(cfg);
        for m in targets {
            self.run_mapping_tracked(&m, Trigger::Poll).await;
        }
    }

    /// 统一入口：跑一次 cycle、持久化状态；Error(自动触发)时按退避安排重试
    pub async fn run_one(self: &Arc<Self>, mapping_id: &str, trigger: Trigger) -> MappingStatus {
        let cfg = self.ctx.config.read();
        let target = cfg.mappings.iter().find(|m| m.id == mapping_id).cloned();
        drop(cfg);
        let Some(m) = target else {
            self.record_status(
                mapping_id,
                MappingStatus::Error,
                Some("映射不存在".into()),
            );
            return MappingStatus::Error;
        };
        self.run_mapping_tracked(&m, trigger).await
    }

    /// 内部：执行 + 记录 + 自动触发失败时安排退避重试
    async fn run_mapping_tracked(self: &Arc<Self>, m: &Mapping, trigger: Trigger) -> MappingStatus {
        let status = run_sync_cycle(&self.ctx, m, trigger).await;
        self.record_status(&m.id, status, None);
        let is_auto = matches!(trigger, Trigger::LocalSettled | Trigger::Poll);
        if is_auto && status == MappingStatus::Error {
            let attempt = {
                let mut map = self.retry_attempts.lock().expect("retry lock poisoned");
                let next = map.get(&m.id).copied().unwrap_or(0) + 1;
                map.insert(m.id.clone(), next);
                next
            };
            self.schedule_retry(m.id.clone(), attempt);
        } else if status == MappingStatus::Idle {
            self.retry_attempts
                .lock()
                .expect("retry lock poisoned")
                .remove(&m.id);
        }
        status
    }

    /// 退避重试（独立方法避免 spawn 块与 tracked 形成 Send 循环依赖）
    fn schedule_retry(self: &Arc<Self>, mapping_id: String, attempt: u32) {
        let engine = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(crate::sync::engine::backoff_delay(attempt)).await;
            if !engine.is_global_paused() {
                engine.run_one(&mapping_id, Trigger::Poll).await;
            }
        });
    }

    pub fn set_global_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
    }

    pub fn is_global_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// 配置/防抖值变更后重建 watcher
    pub fn apply_watch_set(&self) -> Result<(), String> {
        let cfg = self.ctx.config.read();
        let files: Vec<PathBuf> = cfg
            .mappings
            .iter()
            .filter(|m| m.enabled)
            .map(|m| PathBuf::from(&m.local_path))
            .collect();
        let window = Duration::from_secs(cfg.settings.debounce_seconds.clamp(1, 3600));
        drop(cfg);

        let watcher =
            FileWatcher::start(files, window, self.settled_tx.clone()).map_err(|e| e.to_string())?;
        *self.watcher.lock().expect("watcher lock poisoned") = Some(watcher);
        Ok(())
    }

    /// 当前全部持久化状态快照（mapping_list join 用）
    pub fn statuses_snapshot(&self) -> HashMap<String, PersistedStatus> {
        self.statuses
            .lock()
            .expect("statuses lock poisoned")
            .clone()
    }

    fn persist_statuses(&self, map: &HashMap<String, PersistedStatus>) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(map)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let tmp = self.statuses_path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(tmp, &self.statuses_path)
    }

    fn record_status(&self, mapping_id: &str, status: MappingStatus, message: Option<String>) {
        let mut map = self.statuses.lock().expect("statuses lock poisoned");
        let last_synced_at = map
            .get(mapping_id)
            .and_then(|s| s.last_synced_at);
        let synced = matches!(status, MappingStatus::Idle);
        map.insert(
            mapping_id.to_string(),
            PersistedStatus {
                status,
                message,
                last_synced_at: if synced { Some(Utc::now()) } else { last_synced_at },
            },
        );
        // 注意：锁内直接落盘（复用已持有的 map，避免重入死锁）
        if let Err(e) = self.persist_statuses(&map) {
            tracing::warn!("持久化同步状态失败: {e}");
        }
    }
}

/// Windows 路径大小写不敏感比较
fn path_eq(a: &std::path::Path, b: &std::path::Path) -> bool {
    a.to_string_lossy().to_ascii_lowercase() == b.to_string_lossy().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigStore;
    use crate::model::Account;
    use crate::provider::{GistDetail, GistFile, GistProvider, GistSummary, ProviderError, UserInfo};
    use crate::sync::engine::StatusEvent;
    use crate::sync::snapshot::{SelfWriteGuard, SnapshotState, SnapshotStore};
    use crate::sync::{compare, snapshot};
    use std::fs;

    struct FakeProvider {
        files: Mutex<Vec<(String, String, String)>>,
        calls: Mutex<Vec<&'static str>>,
    }

    impl FakeProvider {
        fn new() -> Self {
            Self { files: Mutex::new(vec![]), calls: Mutex::new(vec![]) }
        }
        fn set_file(&self, gist_id: &str, filename: &str, content: &str) {
            self.files.lock().unwrap().retain(|(g, f, _)| !(g == gist_id && f == filename));
            self.files.lock().unwrap().push((gist_id.into(), filename.into(), content.into()));
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
            let files = self
                .files
                .lock()
                .unwrap()
                .iter()
                .filter(|(g, _, _)| g == gist_id)
                .map(|(_, f, c)| GistFile { filename: f.clone(), raw_url: None, content: Some(c.clone()) })
                .collect();
            Ok(GistDetail { id: gist_id.into(), description: None, updated_at: Utc::now(), files })
        }
        async fn update_file(&self, gist_id: &str, filename: &str, content: &str) -> Result<GistDetail, ProviderError> {
            self.calls.lock().unwrap().push("update");
            self.set_file(gist_id, filename, content);
            self.get_gist(gist_id).await
        }
    }

    #[derive(Default)]
    struct VecSink(Mutex<Vec<StatusEvent>>);
    impl crate::sync::engine::StatusSink for VecSink {
        fn emit(&self, event: StatusEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    fn sha(s: &str) -> String {
        compare::sha256_hex(&compare::normalize(s))
    }

    struct Fixture {
        engine: Arc<Engine>,
        fake: Arc<FakeProvider>,
        local_path: PathBuf,
        statuses_path: PathBuf,
        #[allow(dead_code)]
        dir: tempfile::TempDir,
    }

    fn fixture(local: &str, remote: &str, snapshot_content: Option<&str>) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let local_path = dir.path().join("notes.md");
        fs::write(&local_path, local).unwrap();

        let config = ConfigStore::load_from(dir.path().join("config.json")).unwrap();
        config
            .update(|c| {
                c.accounts.push(Account {
                    id: "acc1".into(),
                    name: "测试".into(),
                    provider: crate::model::ProviderKind::Github,
                    base_url: String::new(),
                    token_ref: String::new(),
                });
                c.mappings.push(Mapping {
                    id: "m1".into(),
                    local_path: local_path.to_string_lossy().to_string(),
                    account_id: "acc1".into(),
                    gist_id: "gist1".into(),
                    gist_filename: "notes.md".into(),
                    upload_enabled: true,
                    download_enabled: true,
                    enabled: true,
                });
            })
            .unwrap();

        let snapshots = Arc::new(SnapshotStore::new(dir.path().join("snapshots")));
        if let Some(s) = snapshot_content {
            snapshots
                .save(
                    "m1",
                    &SnapshotState {
                        last_synced_hash: sha(s),
                        last_synced_at: Utc::now(),
                        last_direction: Some(snapshot::SyncDirection::Upload),
                        remote_updated_at: None,
                    },
                )
                .unwrap();
        }
        let fake = Arc::new(FakeProvider::new());
        fake.set_file("gist1", "notes.md", remote);
        let providers = crate::sync::engine::ProviderCache::new();
        providers.insert("acc1", fake.clone());
        let ctx = Arc::new(SyncCtx {
            config: Arc::new(config),
            snapshots,
            providers,
            guard: SelfWriteGuard::new(),
            sink: Arc::new(VecSink::default()),
        });
        let statuses_path = dir.path().join("statuses.json");
        let engine = Arc::new(Engine::new(ctx, statuses_path.clone()).unwrap());
        Fixture { engine, fake, local_path, statuses_path, dir }
    }

    #[tokio::test]
    async fn reconcile_pulls_remote_change() {
        let f = fixture("old local", "new remote", Some("old local"));
        f.engine.startup_reconcile().await;
        assert_eq!(fs::read_to_string(&f.local_path).unwrap(), "new remote");
    }

    #[tokio::test]
    async fn global_pause_blocks_settled_events() {
        let f = fixture("local v2", "base", Some("base"));
        f.engine.set_global_paused(true);
        let calls_before = f.fake.calls.lock().unwrap().len();
        f.engine.dispatch_settled(f.local_path.clone()).await;
        assert_eq!(f.fake.calls.lock().unwrap().len(), calls_before, "暂停时不应触发同步");
        assert!(f.engine.is_global_paused());
    }

    #[tokio::test]
    async fn manual_works_while_paused() {
        let f = fixture("local v2", "base", Some("base"));
        f.engine.set_global_paused(true);
        let status = f.engine.run_one("m1", Trigger::ManualUpload).await;
        assert_eq!(status, MappingStatus::Idle);
        assert!(!fs::read_to_string(&f.local_path).unwrap().is_empty());
    }

    #[tokio::test]
    async fn conflict_persists_across_restart() {
        let f = fixture("local v2", "remote v2", Some("base"));
        let status = f.engine.run_one("m1", Trigger::Poll).await;
        assert_eq!(status, MappingStatus::Conflict);
        assert!(f.statuses_path.exists(), "statuses.json 应已落盘");

        // 重新构造（模拟重启）
        let ctx = f.engine.ctx.clone();
        let reloaded = Engine::new(ctx, f.statuses_path.clone()).unwrap();
        let statuses = reloaded.statuses_snapshot();
        assert_eq!(statuses.get("m1").map(|s| s.status), Some(MappingStatus::Conflict));
    }

    #[tokio::test]
    async fn run_one_missing_mapping_returns_error_status() {
        let f = fixture("local", "remote", None);
        let status = f.engine.run_one("no-such-mapping", Trigger::Poll).await;
        assert_eq!(status, MappingStatus::Error);
    }
}
