use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::model::Mapping;
use crate::sync::engine::{run_sync_cycle_with_hint, MappingStatus, SyncCtx, Trigger};
use crate::sync::watcher::{DebounceWindow, FileWatcher};

/// 持久化状态（statuses.json），重启后恢复 Conflict/Error/Missing
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedStatus {
    pub status: MappingStatus,
    pub message: Option<String>,
    pub last_synced_at: Option<DateTime<Utc>>,
}

/// statuses.json 的文件结构：全局暂停标记 + 各映射状态。
/// 旧版本文件是裸的 `{<mapping_id>: PersistedStatus}` map，加载时兼容回退。
/// deny_unknown_fields：旧格式（顶层是 mapping id）在此解析失败，才能触发回退分支，
/// 否则 serde(default) 会把它静默解析成空 mappings、丢掉全部旧状态。
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusesFile {
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    mappings: HashMap<String, PersistedStatus>,
}

/// 引擎服务：装配 watcher 防抖、轮询、重试队列与状态持久化
pub struct Engine {
    pub ctx: Arc<SyncCtx>,
    statuses: Mutex<HashMap<String, PersistedStatus>>,
    statuses_path: PathBuf,
    paused: AtomicBool,
    retry_attempts: Mutex<HashMap<String, u32>>,
    last_poll: Mutex<HashMap<String, DateTime<Utc>>>,
    /// per-mapping 同步互斥：自动触发（watcher/轮询/重试）try_lock 跳过进行中的映射，
    /// 手动触发排队等待（用户点击的覆盖操作必须执行）
    busy: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    watcher: Mutex<Option<FileWatcher>>,
    /// 防抖管道入口（常驻，watcher 重建只换生产端）：notify 事件 → 防抖 → settled_tx
    debounce_in: mpsc::Sender<PathBuf>,
    debounce_rx: Mutex<Option<mpsc::Receiver<PathBuf>>>,
    debounce_window: DebounceWindow,
    settled_tx: mpsc::Sender<PathBuf>,
    settled_rx: Mutex<Option<mpsc::Receiver<PathBuf>>>,
}

impl Engine {
    /// 构造并加载 statuses.json（不存在 → 空）。
    /// 新格式 {"paused":bool,"mappings":{..}}；旧格式为裸 map（paused 视为 false）。
    pub fn new(ctx: Arc<SyncCtx>, statuses_path: PathBuf) -> std::io::Result<Self> {
        let (paused, statuses) = std::fs::read_to_string(&statuses_path)
            .ok()
            .and_then(|t| {
                serde_json::from_str::<StatusesFile>(&t)
                    .map(|f| (f.paused, f.mappings))
                    .or_else(|_| {
                        serde_json::from_str::<HashMap<String, PersistedStatus>>(&t)
                            .map(|m| (false, m))
                    })
                    .ok()
            })
            .unwrap_or((false, HashMap::new()));
        let (debounce_in, debounce_rx) = mpsc::channel(256);
        let (settled_tx, settled_rx) = mpsc::channel(256);
        let debounce_window = Arc::new(AtomicU64::new(
            ctx.config.read().settings.debounce_seconds,
        ));
        Ok(Self {
            ctx,
            statuses: Mutex::new(statuses),
            statuses_path,
            paused: AtomicBool::new(paused),
            retry_attempts: Mutex::new(HashMap::new()),
            last_poll: Mutex::new(HashMap::new()),
            busy: Mutex::new(HashMap::new()),
            watcher: Mutex::new(None),
            debounce_in,
            debounce_rx: Mutex::new(Some(debounce_rx)),
            debounce_window,
            settled_tx,
            settled_rx: Mutex::new(Some(settled_rx)),
        })
    }

    /// 启动常驻任务：防抖管道 + settled 分发器 + 轮询循环。
    /// 必须在 tokio 运行时上下文中调用（这里 spawn 常驻任务）。
    /// watcher 失败只降级（轮询/对账照常），不整体瘫痪。
    pub fn start(self: &Arc<Self>) -> Result<(), String> {
        // 防抖管道：常驻一次，watcher 重建只换事件生产端（窗口动态读，改设置不重建）
        let debounce_rx = self
            .debounce_rx
            .lock()
            .expect("debounce_rx lock poisoned")
            .take();
        if let Some(rx) = debounce_rx {
            let window = self.debounce_window.clone();
            let out = self.settled_tx.clone();
            tokio::spawn(crate::sync::watcher::debounce_stream(rx, window, out));
        }

        if let Err(e) = self.apply_watch_set() {
            tracing::error!("文件监听启动失败（本轮降级为仅轮询）: {e}");
        }

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
            match provider.list_gists(since).await {
                Ok(list) => {
                    for g in list {
                        changed.insert((account.id.clone(), g.id));
                    }
                    // 仅成功时推进 since：失败窗口的变更留给下一轮全量补偿
                    self.last_poll
                        .lock()
                        .expect("last_poll lock poisoned")
                        .insert(account.id.clone(), Utc::now());
                }
                Err(e) => {
                    tracing::warn!("[{}] 轮询列表失败（since 不推进）: {e}", account.id);
                }
            }
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

    /// 内部：执行 + 记录 + 自动触发失败时安排退避重试（限流错误优先用 Retry-After）
    async fn run_mapping_tracked(self: &Arc<Self>, m: &Mapping, trigger: Trigger) -> MappingStatus {
        // per-mapping 互斥：手动覆盖操作排队等待；自动触发遇到进行中的同步直接跳过本轮
        // （watcher 下次事件 / 轮询下轮 / 重试队列都有补偿，无需重复请求）
        let lock = self
            .busy
            .lock()
            .expect("busy lock poisoned")
            .entry(m.id.clone())
            .or_default()
            .clone();
        let _guard = match trigger {
            Trigger::ManualUpload | Trigger::ManualDownload => lock.lock().await,
            _ => match lock.try_lock() {
                Ok(g) => g,
                Err(_) => {
                    tracing::debug!("[{}] 同步进行中，跳过本轮自动触发", m.id);
                    return self
                        .statuses_snapshot()
                        .get(&m.id)
                        .map(|s| s.status)
                        .unwrap_or(MappingStatus::Idle);
                }
            },
        };

        let outcome = run_sync_cycle_with_hint(&self.ctx, m, trigger).await;
        self.record_status(&m.id, outcome.status, outcome.message);
        let status = outcome.status;
        let is_auto = matches!(trigger, Trigger::LocalSettled | Trigger::Poll);
        if is_auto && status == MappingStatus::Error {
            let attempt = {
                let mut map = self.retry_attempts.lock().expect("retry lock poisoned");
                let next = map.get(&m.id).copied().unwrap_or(0) + 1;
                map.insert(m.id.clone(), next);
                next
            };
            self.schedule_retry(m.id.clone(), attempt, outcome.rate_hint);
        } else if status == MappingStatus::Idle {
            self.retry_attempts
                .lock()
                .expect("retry lock poisoned")
                .remove(&m.id);
        }
        status
    }

    /// 退避重试（独立方法避免 spawn 块与 tracked 形成 Send 循环依赖）
    fn schedule_retry(self: &Arc<Self>, mapping_id: String, attempt: u32, hint: Option<Duration>) {
        let delay = hint
            .filter(|d| !d.is_zero())
            .unwrap_or_else(|| crate::sync::engine::backoff_delay(attempt));
        let engine = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            if !engine.is_global_paused() {
                engine.run_one(&mapping_id, Trigger::Poll).await;
            }
        });
    }

    pub fn set_global_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
        // 全局暂停随 statuses.json 持久化，重启后保持暂停
        let map = self.statuses.lock().expect("statuses lock poisoned").clone();
        if let Err(e) = self.persist_statuses(&map) {
            tracing::warn!("持久化全局暂停状态失败: {e}");
        }
    }

    pub fn is_global_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// 配置变更后重建 notify watcher（只换事件生产端，防抖管道常驻）。
    /// 纯同步、无 runtime 依赖——从 Tauri 同步命令（主线程）调用安全。
    pub fn apply_watch_set(&self) -> Result<(), String> {
        let cfg = self.ctx.config.read();
        let files: Vec<PathBuf> = cfg
            .mappings
            .iter()
            .filter(|m| m.enabled)
            .map(|m| PathBuf::from(&m.local_path))
            .collect();
        drop(cfg);

        let watcher = FileWatcher::start(files, self.debounce_in.clone())
            .map_err(|e| e.to_string())?;
        *self.watcher.lock().expect("watcher lock poisoned") = Some(watcher);
        Ok(())
    }

    /// 更新防抖窗口（settings_update 用）：原子写，防抖循环下一轮事件生效，不重建 watcher
    pub fn set_debounce_window(&self, secs: u64) {
        self.debounce_window.store(secs, Ordering::Relaxed);
    }

    /// 当前全部持久化状态快照（mapping_list join 用）
    pub fn statuses_snapshot(&self) -> HashMap<String, PersistedStatus> {
        self.statuses
            .lock()
            .expect("statuses lock poisoned")
            .clone()
    }

    fn persist_statuses(&self, map: &HashMap<String, PersistedStatus>) -> std::io::Result<()> {
        let file = StatusesFile {
            paused: self.paused.load(Ordering::SeqCst),
            mappings: map.clone(),
        };
        let json = serde_json::to_string_pretty(&file)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let tmp = self.statuses_path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(tmp, &self.statuses_path)
    }

    fn record_status(&self, mapping_id: &str, status: MappingStatus, message: Option<String>) {
        let mut map = self.statuses.lock().expect("statuses lock poisoned");
        let (prev_message, last_synced_at) = map
            .get(mapping_id)
            .map(|s| (s.message.clone(), s.last_synced_at))
            .unwrap_or((None, None));
        let synced = matches!(status, MappingStatus::Idle);
        // Idle = 恢复正常，清空历史消息；其余状态 message 为 None 时沿用旧值
        // （中间态/轮询对账不携带消息，避免把冲突原因等 Tooltip 信息冲掉）
        let message = if synced {
            None
        } else {
            message.or(prev_message)
        };
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

    /// 删除映射的持久化状态（映射删除时级联清理，避免 statuses.json 残条）
    pub fn remove_status(&self, mapping_id: &str) {
        let mut map = self.statuses.lock().expect("statuses lock poisoned");
        // 同步清理互斥锁条目，映射不再回来
        self.busy
            .lock()
            .expect("busy lock poisoned")
            .remove(mapping_id);
        if map.remove(mapping_id).is_some()
            && let Err(e) = self.persist_statuses(&map)
        {
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
        list_since: Mutex<Vec<Option<DateTime<Utc>>>>,
        fail_list: Mutex<bool>,
    }

    impl FakeProvider {
        fn new() -> Self {
            Self {
                files: Mutex::new(vec![]),
                calls: Mutex::new(vec![]),
                list_since: Mutex::new(vec![]),
                fail_list: Mutex::new(false),
            }
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
        async fn list_gists(&self, since: Option<DateTime<Utc>>) -> Result<Vec<GistSummary>, ProviderError> {
            if *self.fail_list.lock().unwrap() {
                return Err(ProviderError::Network("list boom".into()));
            }
            self.list_since.lock().unwrap().push(since);
            let mut ids: Vec<String> = self
                .files
                .lock()
                .unwrap()
                .iter()
                .map(|(g, _, _)| g.clone())
                .collect();
            ids.sort();
            ids.dedup();
            Ok(ids
                .into_iter()
                .map(|id| GistSummary { id, description: None, updated_at: Utc::now(), files: vec![] })
                .collect())
        }
        async fn get_gist(
            &self,
            gist_id: &str,
            _wanted_file: Option<&str>,
        ) -> Result<GistDetail, ProviderError> {
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
            self.get_gist(gist_id, Some(filename)).await
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
    async fn conflict_message_persists_and_clears_on_resolve() {
        let f = fixture("local v2", "remote v2", Some("base"));
        let status = f.engine.run_one("m1", Trigger::Poll).await;
        assert_eq!(status, MappingStatus::Conflict);
        // 冲突原因应随状态持久化（此前恒被 None 覆盖丢失）
        let s = f.engine.statuses_snapshot();
        assert!(
            s.get("m1").and_then(|x| x.message.as_deref()).unwrap_or_default().contains("未同步修改"),
            "冲突消息应持久化，实际: {:?}",
            s.get("m1").map(|x| x.message.clone())
        );

        // 重新构造（模拟重启）：消息仍在
        let ctx = f.engine.ctx.clone();
        let reloaded = Engine::new(ctx, f.statuses_path.clone()).unwrap();
        let s = reloaded.statuses_snapshot();
        assert!(s["m1"].message.is_some(), "重启后冲突消息应保留");

        // 人工解决（远端覆盖本地）→ Idle 后消息清空
        let reloaded = Arc::new(reloaded);
        let status = reloaded.run_one("m1", Trigger::ManualDownload).await;
        assert_eq!(status, MappingStatus::Idle);
        let s = reloaded.statuses_snapshot();
        assert!(s["m1"].message.is_none(), "恢复正常后应清空历史消息");
    }

    #[tokio::test]
    async fn global_pause_persists_across_restart() {
        let f = fixture("local", "remote", None);
        f.engine.set_global_paused(true);
        assert!(f.statuses_path.exists(), "暂停状态应触发 statuses.json 落盘");

        let ctx = f.engine.ctx.clone();
        let reloaded = Engine::new(ctx, f.statuses_path.clone()).unwrap();
        assert!(reloaded.is_global_paused(), "重启后应保持全局暂停");

        reloaded.set_global_paused(false);
        let reloaded2 = Engine::new(reloaded.ctx.clone(), f.statuses_path.clone()).unwrap();
        assert!(!reloaded2.is_global_paused(), "恢复后重启应不再暂停");
    }

    #[tokio::test]
    async fn legacy_statuses_format_loads() {
        // 旧版裸 map 格式：加载不炸、映射状态保留、paused 视为 false
        let f = fixture("local v2", "remote v2", Some("base"));
        let status = f.engine.run_one("m1", Trigger::Poll).await;
        assert_eq!(status, MappingStatus::Conflict);

        // 手工改写为旧格式（裸 map）
        let snapshot = f.engine.statuses_snapshot();
        std::fs::write(&f.statuses_path, serde_json::to_string(&snapshot).unwrap()).unwrap();

        let ctx = f.engine.ctx.clone();
        let reloaded = Engine::new(ctx, f.statuses_path.clone()).unwrap();
        assert!(!reloaded.is_global_paused());
        let s = reloaded.statuses_snapshot();
        assert_eq!(s.get("m1").map(|x| x.status), Some(MappingStatus::Conflict));
    }

    #[tokio::test]
    async fn remove_status_drops_entry_from_disk() {
        let f = fixture("local v2", "remote v2", Some("base"));
        let status = f.engine.run_one("m1", Trigger::Poll).await;
        assert_eq!(status, MappingStatus::Conflict);

        f.engine.remove_status("m1");
        assert!(!f.engine.statuses_snapshot().contains_key("m1"));

        let ctx = f.engine.ctx.clone();
        let reloaded = Engine::new(ctx, f.statuses_path.clone()).unwrap();
        assert!(!reloaded.statuses_snapshot().contains_key("m1"), "删除后磁盘上不应有残条");
    }

    #[tokio::test]
    async fn auto_trigger_skipped_while_mapping_busy() {
        let f = fixture("local v2", "base", Some("base"));
        // 占住 m1 的同步互斥锁，模拟一次进行中的同步
        let lock = f
            .engine
            .busy
            .lock()
            .unwrap()
            .entry("m1".into())
            .or_default()
            .clone();
        let _guard = lock.lock().await;

        let calls_before = f.fake.calls.lock().unwrap().len();
        let status = f.engine.run_one("m1", Trigger::Poll).await;
        assert_eq!(status, MappingStatus::Idle, "跳过时返回当前持久化状态");
        assert_eq!(
            f.fake.calls.lock().unwrap().len(),
            calls_before,
            "自动触发在同步进行中不应重复执行"
        );
    }

    #[tokio::test]
    async fn manual_trigger_queues_behind_busy_lock() {
        let f = fixture("local v2", "base", Some("base"));
        let lock = f
            .engine
            .busy
            .lock()
            .unwrap()
            .entry("m1".into())
            .or_default()
            .clone();
        let _guard = lock.lock().await;

        let engine = f.engine.clone();
        let task = tokio::spawn(async move { engine.run_one("m1", Trigger::ManualUpload).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!task.is_finished(), "手动触发应排队等待而非被跳过");

        drop(_guard);
        let status = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("释放锁后手动触发应完成")
            .unwrap();
        assert_eq!(status, MappingStatus::Idle);
        let remote = f
            .fake
            .files
            .lock()
            .unwrap()
            .iter()
            .find(|(g, _, _)| g == "gist1")
            .map(|(_, _, c)| c.clone());
        assert_eq!(remote.as_deref(), Some("local v2"), "排队的手动上传最终应执行");
    }

    #[tokio::test]
    async fn run_one_missing_mapping_returns_error_status() {
        let f = fixture("local", "remote", None);
        let status = f.engine.run_one("no-such-mapping", Trigger::Poll).await;
        assert_eq!(status, MappingStatus::Error);
    }

    // ── 终审修复通道用例 ──

    #[tokio::test]
    async fn poll_list_failure_keeps_since_none() {
        let f = fixture("old", "new remote", Some("old"));
        *f.fake.fail_list.lock().unwrap() = true;
        f.engine.poll_round().await;
        *f.fake.fail_list.lock().unwrap() = false;
        f.engine.poll_round().await;

        let sinces = f.fake.list_since.lock().unwrap().clone();
        assert_eq!(sinces.len(), 1, "失败轮不应记录 since");
        assert_eq!(sinces[0], None, "首轮失败后 since 不得推进，第二轮应全量补偿");
        // 第二轮以 None 全量拉取 → 命中变更 → 本地已下载
        assert_eq!(fs::read_to_string(&f.local_path).unwrap(), "new remote");
    }

    /// schedule_retry 的 hint 优先逻辑（hint 非零则覆盖指数退避）
    #[test]
    fn retry_hint_overrides_backoff() {
        let hint = Some(Duration::from_secs(42));
        let attempt = 3;
        let delay = hint
            .filter(|d| !d.is_zero())
            .unwrap_or_else(|| crate::sync::engine::backoff_delay(attempt));
        assert_eq!(delay, Duration::from_secs(42));
        let none: Option<Duration> = None;
        let delay2 = none
            .filter(|d| !d.is_zero())
            .unwrap_or_else(|| crate::sync::engine::backoff_delay(3));
        assert_eq!(delay2, Duration::from_secs(4));
    }

    /// 回归测试：apply_watch_set 必须可在非 tokio runtime 线程调用
    /// （Tauri 同步命令在主线程执行；曾经其内部 tokio::spawn 导致 panic=abort 闪退，
    /// 见 doc/20261014-bug报告-配置变更闪退.md）
    #[test]
    fn apply_watch_set_works_outside_tokio_runtime() {
        let f = fixture("local", "remote", None);
        f.engine.apply_watch_set().expect("非 runtime 线程重建 watcher 不应 panic");
    }

    #[test]
    fn set_debounce_window_stores_without_rebuild() {
        let f = fixture("local", "remote", None);
        f.engine.set_debounce_window(30);
        assert_eq!(
            f.engine.debounce_window.load(Ordering::Relaxed),
            30
        );
    }
}
