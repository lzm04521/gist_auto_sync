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
use crate::sync::history::HistoryReason;
use crate::sync::snapshot::{SelfWriteGuard, Side, SnapshotState, SnapshotStore};

/// 触发源
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
}

/// 单次循环结果：最终状态 + 最终态消息（随状态持久化）+ 限流重试提示
#[derive(Debug, Clone)]
pub struct CycleOutcome {
    pub status: MappingStatus,
    pub message: Option<String>,
    pub rate_hint: Option<Duration>,
    /// 真正发生写入（上传/下载成功）时的变化统计；None=本次循环无写入
    pub write: Option<WriteStat>,
}

/// 一次写入的变化统计（added/removed 为 None 表示内容超限未统计）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WriteStat {
    pub direction: crate::synclog::LogDirection,
    pub added: Option<u32>,
    pub removed: Option<u32>,
}

impl CycleOutcome {
    fn plain(status: MappingStatus) -> Self {
        Self { status, message: None, rate_hint: None, write: None }
    }
}

/// 行级变化统计：先与哈希比较同基准归一化行尾，再逐行 diff，
/// 返回 (新增行数, 删除行数)；内容合计超 4MB 不统计（防大文件 diff 卡顿）
pub fn line_diff(old: &str, new: &str) -> Option<(u32, u32)> {
    const SIZE_CAP: usize = 4 * 1024 * 1024;
    if old.len() + new.len() > SIZE_CAP {
        return None;
    }
    let old_n = normalize(old);
    let new_n = normalize(new);
    let diff = similar::TextDiff::from_lines(old_n.as_str(), new_n.as_str());
    let (mut added, mut removed) = (0u32, 0u32);
    for change in diff.iter_all_changes() {
        match change.tag() {
            similar::ChangeTag::Insert => added += 1,
            similar::ChangeTag::Delete => removed += 1,
            similar::ChangeTag::Equal => {}
        }
    }
    Some((added, removed))
}

/// 由 line_diff 组装 WriteStat（超限未统计时 added/removed 均为 None）
fn write_stat(direction: crate::synclog::LogDirection, old: &str, new: &str) -> WriteStat {
    match line_diff(old, new) {
        Some((added, removed)) => WriteStat { direction, added: Some(added), removed: Some(removed) },
        None => WriteStat { direction, added: None, removed: None },
    }
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
    /// 同步操作流水（V0.5）：引擎出口按规则落库
    pub synclog: Arc<crate::synclog::SyncLogStore>,
}

/// 单次同步循环（spec §6.1/6.2/6.5）。返回最终状态；过程经 sink 推送。
pub async fn run_sync_cycle(ctx: &SyncCtx, mapping: &Mapping, trigger: Trigger) -> MappingStatus {
    run_sync_cycle_with_hint(ctx, mapping, trigger).await.status
}

/// 同 run_sync_cycle，额外返回最终态消息与限流重试提示（RateLimited.retry_after）
pub async fn run_sync_cycle_with_hint(
    ctx: &SyncCtx,
    mapping: &Mapping,
    trigger: Trigger,
) -> CycleOutcome {
    let (local_raw, local_mtime) = match std::fs::read_to_string(&mapping.local_path) {
        Ok(text) => {
            let mtime = std::fs::metadata(&mapping.local_path)
                .ok()
                .and_then(|m| m.modified().ok());
            (Some(text), mtime)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (None, None),
        Err(e) => {
            return emit_outcome(
                ctx,
                mapping,
                MappingStatus::Error,
                status_msg("local.read_failed", format!("读取本地文件失败: {e}")),
                false,
                None,
            );
        }
    };
    let local_hash = local_raw.as_ref().map(|t| sha256_hex(&normalize(t)));

    // ── 自写抑制：下载写盘触发的 watcher 事件直接忽略（spec §6.1）
    if trigger == Trigger::LocalSettled
        && let (Some(h), Some(m)) = (&local_hash, local_mtime)
        && ctx.guard.should_suppress(&mapping.id, h, m)
    {
        return CycleOutcome::plain(MappingStatus::Idle);
    }

    // ── provider 解析
    let account = ctx
        .config
        .read()
        .accounts
        .into_iter()
        .find(|a| a.id == mapping.account_id);
    let Some(account) = account else {
        return emit_outcome(
            ctx,
            mapping,
            MappingStatus::Error,
            status_msg("account.not_found", "账号不存在"),
            false,
            None,
        );
    };
    let provider = match ctx.providers.get_or_build(&account).await {
        Ok(p) => p,
        Err(e) => {
            return emit_outcome(
                ctx,
                mapping,
                MappingStatus::Error,
                status_msg("provider.build_failed", e.to_string()),
                false,
                rate_limit_hint(&e),
            )
        }
    };

    // ── 远端读取（只回补目标文件的 content，避免为 gist 内无关文件拉 raw）
    let detail = match provider
        .get_gist(&mapping.gist_id, Some(&mapping.gist_filename))
        .await
    {
        Ok(d) => d,
        Err(e) => {
            return emit_outcome(
                ctx,
                mapping,
                MappingStatus::Error,
                status_msg("provider.get_failed", e.to_string()),
                false,
                rate_limit_hint(&e),
            )
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

    // ── 手动语义改写：显式覆盖意图优先于自动决策（spec：Manual 兼作冲突解决与 Missing 恢复）
    let decision = match trigger {
        Trigger::ManualUpload => {
            let Some(local) = &local_raw else {
                return emit_outcome(
                    ctx,
                    mapping,
                    MappingStatus::Error,
                    status_msg("manual.local_missing", "本地文件不存在"),
                    false,
                    None,
                );
            };
            let local_changed = snapshot
                .as_ref()
                .map(|s| s.last_synced_hash != local_hash.clone().unwrap())
                .unwrap_or(true);
            let _ = local;
            // 远端文件缺失（被改名/删除）时也放行：ManualUpload = 在远端重建（spec §6.3）
            if !local_changed && remote_file.is_some() {
                return emit_outcome(
                    ctx,
                    mapping,
                    MappingStatus::Idle,
                    status_msg("manual.local_unchanged", "本地无未同步修改"),
                    false,
                    None,
                );
            }
            Decision::Upload
        }
        Trigger::ManualDownload => {
            let Some(remote) = &remote_raw else {
                return emit_outcome(
                    ctx,
                    mapping,
                    MappingStatus::Error,
                    status_msg("manual.remote_missing", "远端不存在该文件"),
                    false,
                    None,
                );
            };
            let remote_changed = snapshot
                .as_ref()
                .map(|s| s.last_synced_hash != remote_hash.clone().unwrap())
                .unwrap_or(true);
            let _ = remote;
            // 本地文件缺失（被删除）时也放行：ManualDownload = 恢复本地（spec §6.3）
            if !remote_changed && local_raw.is_some() {
                return emit_outcome(
                    ctx,
                    mapping,
                    MappingStatus::Idle,
                    status_msg("manual.remote_unchanged", "远端无变更"),
                    false,
                    None,
                );
            }
            Decision::Download
        }
        _ => decide(&input),
    };

    let is_auto = matches!(trigger, Trigger::LocalSettled | Trigger::Poll);

    match decision {
        Decision::Noop => CycleOutcome::plain(MappingStatus::Idle),
        Decision::SeedSnapshot => {
            let hash = input.local_hash.clone().unwrap_or_default();
            let state = SnapshotState {
                last_synced_hash: hash,
                last_synced_at: Utc::now(),
                last_direction: None,
                remote_updated_at: Some(detail.updated_at),
            };
            if let Err(e) = ctx.snapshots.save(&mapping.id, &state) {
                tracing::error!("[{}] 建立同步基准失败: {e}", mapping.id);
                return emit_outcome(
                    ctx,
                    mapping,
                    MappingStatus::Error,
                    status_msg("snapshot.save_failed", format!("保存快照失败: {e}")),
                    false,
                    None,
                );
            }
            tracing::info!("[{}] 建立同步基准（首次映射两边一致）", mapping.id);
            emit_outcome(
                ctx,
                mapping,
                MappingStatus::Idle,
                status_msg("baseline.created", "已建立同步基准"),
                true,
                None,
            )
        }
        Decision::MissingLocal => {
            tracing::warn!("[{}] 本地文件不存在或已删除，等待人工恢复", mapping.id);
            emit_outcome(
                ctx,
                mapping,
                MappingStatus::Missing,
                status_msg("local.missing", "本地文件不存在或已删除"),
                false,
                None,
            )
        }
        Decision::MissingRemote => {
            tracing::warn!("[{}] 远端 gist 中不存在该文件，等待人工恢复", mapping.id);
            emit_outcome(
                ctx,
                mapping,
                MappingStatus::Missing,
                status_msg("remote.missing", "远端 gist 中不存在该文件"),
                false,
                None,
            )
        }
        Decision::Conflict => {
            tracing::warn!("[{}] 本地与远端均有未同步修改，已暂停等待人工选择", mapping.id);
            emit_outcome(
                ctx,
                mapping,
                MappingStatus::Conflict,
                status_msg("conflict.dual_change", "本地与远端均有未同步修改，请人工选择保留哪边"),
                false,
                None,
            )
        }
        Decision::Upload => {
            if !mapping.upload_enabled && is_auto {
                return emit_outcome(
                    ctx,
                    mapping,
                    MappingStatus::Idle,
                    status_msg("upload.skipped_disabled", "上传开关已关闭，跳过上传"),
                    false,
                    None,
                );
            }
            emit_status(ctx, mapping, MappingStatus::Uploading, None, false);
            // 归档旧远端（远端文件不存在时无可归档）
            if let Some(remote) = &remote_raw {
                let _ = ctx.snapshots.archive(
                    &mapping.id,
                    Side::Remote,
                    HistoryReason::Upload,
                    remote,
                    &ext_of(&mapping.gist_filename),
                );
            }
            // 上传规范化内容（CRLF→LF），快照以回读为准
            let content = normalize(local_raw.as_deref().expect("Upload 分支必有本地内容"));
            // 变化统计：远端上一版（远端无该文件视为空）→ 本次上传内容
            let stat = write_stat(crate::synclog::LogDirection::Upload, remote_raw.as_deref().unwrap_or(""), &content);
            tracing::info!("[{}] 上传 {} → gist {}（{} 字节）", mapping.id, mapping.gist_filename, mapping.gist_id, content.len());
            match provider
                .update_file(&mapping.gist_id, &mapping.gist_filename, &content)
                .await
            {
                Ok(new_detail) => {
                    // 回读校验（spec：快照以回读为准）：响应 content 缺失或不一致时再取一次远端核对
                    let sent_hash = sha256_hex(&content);
                    let readback_ok = |c: &str| sha256_hex(&normalize(c)) == sent_hash;
                    let mut verified = new_detail
                        .files
                        .iter()
                        .find(|f| f.filename == mapping.gist_filename)
                        .and_then(|f| f.content.clone())
                        .filter(|c| readback_ok(c));
                    if verified.is_none() {
                        tracing::warn!("[{}] 上传响应回读缺失或不一致，重新拉取远端核对", mapping.id);
                        match provider
                            .get_gist(&mapping.gist_id, Some(&mapping.gist_filename))
                            .await
                        {
                            Ok(d) => {
                                verified = d
                                    .files
                                    .iter()
                                    .find(|f| f.filename == mapping.gist_filename)
                                    .and_then(|f| f.content.clone())
                                    .filter(|c| readback_ok(c));
                            }
                            Err(e) => {
                                return emit_outcome(
                                    ctx,
                                    mapping,
                                    MappingStatus::Error,
                                    status_msg("upload.verify_failed", format!("上传回读校验失败: {e}")),
                                    false,
                                    rate_limit_hint(&e),
                                );
                            }
                        }
                    }
                    let Some(back) = verified else {
                        tracing::error!("[{}] 上传回读校验失败：远端内容与发送不一致，不落快照", mapping.id);
                        return emit_outcome(
                            ctx,
                            mapping,
                            MappingStatus::Error,
                            status_msg("upload.verify_failed", "上传回读校验失败：远端内容与发送不一致"),
                            false,
                            None,
                        );
                    };
                    let state = SnapshotState {
                        last_synced_hash: sha256_hex(&normalize(&back)),
                        last_synced_at: Utc::now(),
                        last_direction: Some(crate::sync::snapshot::SyncDirection::Upload),
                        remote_updated_at: Some(new_detail.updated_at),
                    };
                    if let Err(e) = ctx.snapshots.save(&mapping.id, &state) {
                        return emit_outcome(
                            ctx,
                            mapping,
                            MappingStatus::Error,
                            status_msg("snapshot.save_failed", format!("保存快照失败: {e}")),
                            false,
                            None,
                        );
                    }
                    tracing::info!("[{}] 上传完成", mapping.id);
                    emit_outcome_stat(ctx, mapping, MappingStatus::Idle, None, true, None, stat)
                }
                Err(e) => {
                    tracing::error!("[{}] 上传失败: {e}", mapping.id);
                    emit_outcome(
                        ctx,
                        mapping,
                        MappingStatus::Error,
                        status_msg("upload.failed", e.to_string()),
                        false,
                        rate_limit_hint(&e),
                    )
                }
            }
        }
        Decision::Download => {
            if !mapping.download_enabled && is_auto {
                return emit_outcome(
                    ctx,
                    mapping,
                    MappingStatus::Idle,
                    status_msg("download.skipped_disabled", "下载开关已关闭，跳过下载"),
                    false,
                    None,
                );
            }
            emit_status(ctx, mapping, MappingStatus::Downloading, None, false);
            // 归档旧本地
            if let Some(local) = &local_raw {
                let _ = ctx.snapshots.archive(
                    &mapping.id,
                    Side::Local,
                    HistoryReason::Download,
                    local,
                    &ext_of(&mapping.local_path),
                );
            }
            // 写盘保留远端原始字节（spec §6.5）；原子替换（temp + rename 同目录同卷），
            // 写一半失败不会留下截断文件；temp 路径与映射路径不同，watcher 事件天然不命中映射
            let raw = remote_raw.clone().expect("Download 分支必有远端内容");
            // 变化统计：本地上一版（本地无文件视为空）→ 远端内容
            let stat = write_stat(crate::synclog::LogDirection::Download, local_raw.as_deref().unwrap_or(""), &raw);
            tracing::info!("[{}] 下载 gist {}/{} → {}（{} 字节）", mapping.id, mapping.gist_id, mapping.gist_filename, mapping.local_path, raw.len());
            if let Err(e) = write_local_atomic(&mapping.local_path, &raw) {
                return emit_outcome(
                    ctx,
                    mapping,
                    MappingStatus::Error,
                    status_msg("download.write_failed", format!("写入本地文件失败: {e}")),
                    false,
                    None,
                );
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
                return emit_outcome(
                    ctx,
                    mapping,
                    MappingStatus::Error,
                    status_msg("snapshot.save_failed", format!("保存快照失败: {e}")),
                    false,
                    None,
                );
            }
            tracing::info!("[{}] 下载完成", mapping.id);
            emit_outcome_stat(ctx, mapping, MappingStatus::Idle, None, true, None, stat)
        }
    }
}

/// RateLimited 错误 → 重试等待提示
fn rate_limit_hint(e: &ProviderError) -> Option<Duration> {
    match e {
        ProviderError::RateLimited { retry_after } => *retry_after,
        _ => None,
    }
}

pub(crate) fn ext_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_else(|| "txt".to_string())
}

/// 本地原子写盘（temp + rename 同目录同卷）：下载与回滚共用。
/// temp 路径与映射路径不同，watcher 事件天然不命中映射。
pub(crate) fn write_local_atomic(local_path: &str, raw: &str) -> std::io::Result<()> {
    let local = Path::new(local_path);
    let tmp_path = local.with_file_name(format!(
        ".{}.gistsync.tmp",
        local
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    ));
    std::fs::write(&tmp_path, raw.as_bytes())?;
    if let Err(e) = std::fs::rename(&tmp_path, local) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }
    Ok(())
}

pub(crate) fn emit_status(ctx: &SyncCtx, mapping: &Mapping, status: MappingStatus, message: Option<String>, synced: bool) -> MappingStatus {
    ctx.sink.emit(StatusEvent {
        mapping_id: mapping.id.clone(),
        status,
        message,
        last_synced_at: synced.then(Utc::now),
    });
    status
}

/// 引擎状态消息的结构化封装：`{"code":"...","detail":"..."}`。
/// 前端按 code 走 i18n（statusMsg.<code>），detail 为兜底展示（与 IPC 错误同构）。
pub fn status_msg(code: &str, detail: impl Into<String>) -> Option<String> {
    Some(serde_json::json!({ "code": code, "detail": detail.into() }).to_string())
}

/// emit_status 的 CycleOutcome 版本：事件照常推送，最终态 message 一并返回给调用方持久化
fn emit_outcome(
    ctx: &SyncCtx,
    mapping: &Mapping,
    status: MappingStatus,
    message: Option<String>,
    synced: bool,
    rate_hint: Option<Duration>,
) -> CycleOutcome {
    let status = emit_status(ctx, mapping, status, message.clone(), synced);
    CycleOutcome { status, message, rate_hint, write: None }
}

/// emit_outcome 的写入变体：成功写入（上传/下载完成）时附带变化统计
fn emit_outcome_stat(
    ctx: &SyncCtx,
    mapping: &Mapping,
    status: MappingStatus,
    message: Option<String>,
    synced: bool,
    rate_hint: Option<Duration>,
    stat: WriteStat,
) -> CycleOutcome {
    let mut out = emit_outcome(ctx, mapping, status, message, synced, rate_hint);
    out.write = Some(stat);
    out
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
        /// 模拟远端回读与发送不一致（服务端篡改/中间层截断）
        corrupt_update: Mutex<bool>,
        calls: Mutex<Vec<&'static str>>,
    }

    impl FakeProvider {
        fn new() -> Self {
            Self {
                files: Mutex::new(vec![]),
                fail_next: Mutex::new(None),
                corrupt_update: Mutex::new(false),
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
        async fn get_gist(
            &self,
            gist_id: &str,
            _wanted_file: Option<&str>,
        ) -> Result<GistDetail, ProviderError> {
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
            let stored = if *self.corrupt_update.lock().unwrap() {
                format!("corrupted:{content}")
            } else {
                content.to_string()
            };
            self.set_file(gist_id, filename, &stored);
            self.get_gist(gist_id, Some(filename)).await
        }
        async fn list_commits(&self, _gist_id: &str) -> Result<Vec<crate::provider::GistCommit>, ProviderError> {
            Ok(vec![])
        }
        async fn get_file_at_revision(&self, _gist_id: &str, _rev: &str, _filename: &str) -> Result<String, ProviderError> {
            Err(ProviderError::NotFound)
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
            synclog: Arc::new(crate::synclog::SyncLogStore::load_from(
                dir.path().join("synclog.jsonl"),
            )),
        };
        let mapping = Mapping {
            id: "m1".into(),
            name: String::new(),
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

    // ── 变化统计（V0.5）──────────────────────────────────────────────

    #[test]
    fn line_diff_counts_added_removed_and_modified() {
        assert_eq!(line_diff("a\nb\n", "a\nb\nc\n"), Some((1, 0)));
        assert_eq!(line_diff("a\nb\nc\n", "a\nb\n"), Some((0, 1)));
        assert_eq!(line_diff("a\nx\nc\n", "a\ny\nc\n"), Some((1, 1)), "改行计 1+1");
        assert_eq!(line_diff("", "x\ny\n"), Some((2, 0)), "全新内容");
    }

    #[test]
    fn line_diff_ignores_eol_only_change() {
        // 与哈希比较同基准：先归一化行尾，纯 CRLF/LF 差异不算变化
        assert_eq!(line_diff("a\r\nb\r\n", "a\nb\n"), Some((0, 0)));
    }

    #[test]
    fn line_diff_returns_none_over_size_cap() {
        let big = "x\n".repeat(3 * 1024 * 1024);
        assert!(line_diff(&big, "a").is_none(), "超 4MB 不统计");
    }

    #[tokio::test]
    async fn upload_cycle_carries_write_stat() {
        use crate::synclog::LogDirection;
        let f = fixture("a\nb\n", "a\n", Some("a\n"));
        let out = run_sync_cycle_with_hint(&f.ctx, &f.mapping, Trigger::Poll).await;
        let w = out.write.expect("上传成功应带变化统计");
        assert_eq!(w.direction, LogDirection::Upload);
        assert_eq!(w.added, Some(1), "远端 a→a+b：新增 1 行");
        assert_eq!(w.removed, Some(0));
    }

    #[tokio::test]
    async fn download_cycle_carries_write_stat() {
        use crate::synclog::LogDirection;
        let f = fixture("a\n", "a\nb\n", Some("a\n"));
        let out = run_sync_cycle_with_hint(&f.ctx, &f.mapping, Trigger::Poll).await;
        let w = out.write.expect("下载成功应带变化统计");
        assert_eq!(w.direction, LogDirection::Download);
        assert_eq!(w.added, Some(1));
        assert_eq!(w.removed, Some(0));
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
    async fn upload_readback_mismatch_fails_and_keeps_snapshot() {
        // 远端回读与发送不一致（服务端篡改/截断）：报错、不落快照
        let f = fixture("local v2", "base", Some("base"));
        *f.fake.corrupt_update.lock().unwrap() = true;
        let status = run_sync_cycle(&f.ctx, &f.mapping, Trigger::ManualUpload).await;
        assert_eq!(status, MappingStatus::Error);
        assert_eq!(
            f.ctx.snapshots.load("m1").unwrap().last_synced_hash,
            sha("base"),
            "回读校验失败时快照不应推进"
        );
        // 二次核对会多打一次 get（初次 update 响应 1 次 + 重拉 1 次）
        assert!(f.fake.calls() >= 2, "应触发二次拉取核对");
    }

    #[tokio::test]
    async fn download_leaves_no_tmp_file() {
        // 原子写盘：成功后目录里不应残留 .gistsync.tmp 临时文件
        let f = fixture("base", "remote v2", Some("base"));
        let status = run_sync_cycle(&f.ctx, &f.mapping, Trigger::Poll).await;
        assert_eq!(status, MappingStatus::Idle);
        let parent = f.local_path.parent().unwrap();
        let leftovers: Vec<String> = fs::read_dir(parent)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains("gistsync.tmp"))
            .collect();
        assert!(leftovers.is_empty(), "不应有临时文件残留: {leftovers:?}");
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

    // ── 终审修复通道用例 ──

    #[tokio::test]
    async fn manual_upload_recreates_missing_remote() {
        let f = fixture("base", "irrelevant", Some("base"));
        f.fake.remove_file("gist1", "notes.md");
        let status = run_sync_cycle(&f.ctx, &f.mapping, Trigger::ManualUpload).await;
        assert_eq!(status, MappingStatus::Idle);
        assert_eq!(f.fake.update_calls(), 1, "远端文件缺失时 ManualUpload 应在远端重建");
    }

    #[tokio::test]
    async fn manual_download_restores_missing_local() {
        let f = fixture("base", "base", Some("base"));
        fs::remove_file(&f.local_path).unwrap();
        let status = run_sync_cycle(&f.ctx, &f.mapping, Trigger::ManualDownload).await;
        assert_eq!(status, MappingStatus::Idle);
        assert_eq!(fs::read_to_string(&f.local_path).unwrap(), "base", "本地缺失时 ManualDownload 应恢复本地");
    }

    #[tokio::test]
    async fn rate_limited_error_carries_retry_hint() {
        let f = fixture("local", "remote", Some("base"));
        *f.fake.fail_next.lock().unwrap() = Some(ProviderError::RateLimited {
            retry_after: Some(Duration::from_secs(30)),
        });
        let outcome = run_sync_cycle_with_hint(&f.ctx, &f.mapping, Trigger::Poll).await;
        assert_eq!(outcome.status, MappingStatus::Error);
        assert_eq!(outcome.rate_hint, Some(Duration::from_secs(30)));
        assert!(outcome.message.is_some(), "限流错误应携带消息");
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

