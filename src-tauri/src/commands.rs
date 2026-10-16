use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::config::ConfigStore;
use crate::model::{Account, GlobalSettings, Mapping, ProviderKind};
use crate::provider::{GistCommit, GistSummary, UserInfo};
use crate::secrets;
use crate::sync::engine::{MappingStatus, ProviderCache, StatusEvent, StatusSink, Trigger};
use crate::sync::history::HistoryEntry;
use crate::sync::service::{Engine, PersistedStatus};
use crate::sync::snapshot::SnapshotStore;

/// IPC 全局状态（setup 时构造并 manage）
pub struct AppState {
    pub config: Arc<ConfigStore>,
    pub snapshots: Arc<SnapshotStore>,
    pub engine: Arc<Engine>,
    pub synclog: Arc<crate::synclog::SyncLogStore>,
    pub log_dir: std::path::PathBuf,
    /// 日志级别热更新句柄（初始化失败时为 None，级别改动需重启生效）
    pub log_reload: Option<crate::logging::LogReloadHandle>,
}

/// IPC 命令错误统一结构化：`{"code":"...","detail":"..."}`。
/// 前端按 code 走 i18n（errors.<code>），detail 兜底展示（原始错误/补充信息）。
fn ipc_err(code: &str, detail: impl Into<String>) -> String {
    serde_json::json!({ "code": code, "detail": detail.into() }).to_string()
}

/// 把引擎状态事件转发到前端（event: sync://status）
pub struct IpcSink {
    pub app: AppHandle,
}

impl StatusSink for IpcSink {
    fn emit(&self, event: StatusEvent) {
        if let Err(e) = self.app.emit("sync://status", &event) {
            tracing::warn!("推送同步状态事件失败: {e}");
        }
    }
}

// ── DTO（全部 camelCase，与前端 src/types.ts 镜像）──────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInput {
    pub name: String,
    pub provider: ProviderKind,
    pub base_url: String,
    /// 更新时留空表示保留原 token
    pub token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MappingInput {
    /// 显示名（可选，纯展示用途；旧前端缺键为空串）
    #[serde(default)]
    pub name: String,
    pub local_path: String,
    pub account_id: String,
    pub gist_id: String,
    /// 留空默认取本地文件名
    pub gist_filename: String,
    pub upload_enabled: bool,
    pub download_enabled: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 事件脚本钩子（V0.6；旧前端缺键为全空）
    #[serde(default)]
    pub hooks: crate::model::MappingHooks,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MappingView {
    pub mapping: Mapping,
    pub status: MappingStatus,
    pub message: Option<String>,
    pub last_synced_at: Option<chrono::DateTime<chrono::Utc>>,
}

// ── 账号命令 ─────────────────────────────────────────────────────────

#[tauri::command]
pub fn account_list(state: State<'_, AppState>) -> Vec<Account> {
    state.config.read().accounts
}

#[tauri::command]
pub fn account_create(state: State<'_, AppState>, input: AccountInput) -> Result<Account, String> {
    // 输入校验先于凭据写入，非法输入不产生 keyring 残留
    let base_url = match input.provider {
        ProviderKind::Github => "https://api.github.com".to_string(),
        ProviderKind::Opengist => {
            let url = input.base_url.trim().trim_end_matches('/').to_string();
            if !url.starts_with("http") {
                return Err(ipc_err("ACCOUNT_BASE_URL_INVALID", "Opengist 实例地址必须以 http(s):// 开头"));
            }
            url
        }
    };
    let id = uuid::Uuid::new_v4().to_string();
    let token_ref = secrets::token_ref_for(&id);
    secrets::save_token(&token_ref, &input.token)
        .map_err(|e| ipc_err("ACCOUNT_KEYRING_WRITE", e.to_string()))?;

    let account = Account {
        id,
        name: input.name.trim().to_string(),
        provider: input.provider,
        base_url,
        token_ref,
    };
    if let Err(e) = state.config.update(|c| c.accounts.push(account.clone())) {
        // 配置落盘失败：清理已写入的凭据，避免 keyring 孤儿条目（best-effort，失败不掩盖原错误）
        let _ = secrets::delete_token(account.token_ref.as_str());
        return Err(ipc_err("CONFIG_SAVE_FAILED", e.to_string()));
    }
    Ok(account)
}

#[tauri::command]
pub fn account_update(
    state: State<'_, AppState>,
    id: String,
    input: AccountInput,
) -> Result<(), String> {
    let existing = state
        .config
        .read()
        .accounts
        .into_iter()
        .find(|a| a.id == id)
        .ok_or_else(|| ipc_err("ACCOUNT_NOT_FOUND", "账号不存在"))?;
    let token_ref = existing.token_ref.clone();
    if !input.token.trim().is_empty() {
        secrets::save_token(&token_ref, input.token.trim())
            .map_err(|e| ipc_err("ACCOUNT_KEYRING_WRITE", e.to_string()))?;
    }
    let base_url = match input.provider {
        ProviderKind::Github => "https://api.github.com".to_string(),
        ProviderKind::Opengist => {
            let url = input.base_url.trim().trim_end_matches('/').to_string();
            if !url.starts_with("http") {
                return Err(ipc_err("ACCOUNT_BASE_URL_INVALID", "Opengist 实例地址必须以 http(s):// 开头"));
            }
            url
        }
    };
    state
        .config
        .update(|c| {
            if let Some(a) = c.accounts.iter_mut().find(|a| a.id == id) {
                a.name = input.name.trim().to_string();
                a.provider = input.provider;
                a.base_url = base_url;
            }
        })
        .map_err(|e| ipc_err("CONFIG_SAVE_FAILED", e.to_string()))?;
    // token/实例可能变了：缓存失效
    state.ctx_providers().invalidate(&id);
    Ok(())
}

#[tauri::command]
pub fn account_delete(state: State<'_, AppState>, id: String, force: bool) -> Result<(), String> {
    let existing = state
        .config
        .read()
        .accounts
        .into_iter()
        .find(|a| a.id == id)
        .ok_or_else(|| ipc_err("ACCOUNT_NOT_FOUND", "账号不存在"))?;
    let mappings: Vec<Mapping> = state
        .config
        .read()
        .mappings
        .into_iter()
        .filter(|m| m.account_id == id)
        .collect();
    if !mappings.is_empty() && !force {
        // 结构化错误：前端按 code 分支（强删确认框），count 参与文案插值
        return Err(
            serde_json::json!({
                "code": "ACCOUNT_IN_USE",
                "detail": "该账号仍有同步映射",
                "count": mappings.len()
            })
            .to_string(),
        );
    }
    for m in &mappings {
        let _ = state.snapshots.remove_mapping(&m.id);
        state.engine.remove_status(&m.id);
    }
    state
        .config
        .update(|c| {
            c.accounts.retain(|a| a.id != id);
            c.mappings.retain(|m| m.account_id != id);
        })
        .map_err(|e| ipc_err("CONFIG_SAVE_FAILED", e.to_string()))?;
    // token 引用已无意义，尽力清理（失败不阻塞删除）
    let _ = secrets::delete_token(&existing.token_ref);
    state.ctx_providers().invalidate(&id);
    state.engine.apply_watch_set().ok();
    Ok(())
}

#[tauri::command]
pub async fn account_test(state: State<'_, AppState>, id: String) -> Result<UserInfo, String> {
    let account = state
        .config
        .read()
        .accounts
        .into_iter()
        .find(|a| a.id == id)
        .ok_or_else(|| ipc_err("ACCOUNT_NOT_FOUND", "账号不存在"))?;
    let provider = state
        .ctx_providers()
        .get_or_build(&account)
        .await
        .map_err(|e| ipc_err("PROVIDER_REQUEST_FAILED", e.to_string()))?;
    provider
        .verify_token()
        .await
        .map_err(|e| ipc_err("PROVIDER_REQUEST_FAILED", e.to_string()))
}

// ── 映射命令 ─────────────────────────────────────────────────────────

#[tauri::command]
pub fn mapping_list(state: State<'_, AppState>) -> Vec<MappingView> {
    let cfg = state.config.read();
    let statuses = state.engine.statuses_snapshot();
    cfg.mappings
        .into_iter()
        .map(|m| {
            let persisted: PersistedStatus = statuses.get(&m.id).cloned().unwrap_or(PersistedStatus {
                status: MappingStatus::Idle,
                message: None,
                last_synced_at: None,
            });
            MappingView {
                mapping: m,
                status: persisted.status,
                message: persisted.message,
                last_synced_at: persisted.last_synced_at,
            }
        })
        .collect()
}

/// 统一存储路径：canonicalize 并去掉 `\\?\` 前缀；文件不存在则仅统一分隔符。
/// 保证 watcher（notify 以 watch 目录 + `\` 拼接事件路径）能命中手填的正斜杠路径。
fn normalize_stored_path(raw: &str) -> String {
    match std::fs::canonicalize(raw) {
        Ok(p) => p
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .to_string(),
        Err(_) => raw.replace('/', "\\"),
    }
}

/// 映射输入校验：文件存在、UTF-8、localPath 唯一（大小写不敏感）、(账号,gist,文件名) 唯一
fn validate_mapping(
    state: &AppState,
    input: &MappingInput,
    exclude_id: Option<&str>,
) -> Result<(String, String), String> {
    if input.name.chars().count() > 100 {
        return Err(ipc_err("MAPPING_NAME_TOO_LONG", "映射名称长度不能超过 100 字符"));
    }
    if !state
        .config
        .read()
        .accounts
        .iter()
        .any(|a| a.id == input.account_id)
    {
        return Err(ipc_err("ACCOUNT_NOT_FOUND", "所选账号不存在"));
    }
    let path = Path::new(&input.local_path);
    if !path.is_absolute() {
        return Err(ipc_err("MAPPING_LOCAL_ABSOLUTE", "本地路径必须是绝对路径"));
    }
    if !path.is_file() {
        return Err(ipc_err("MAPPING_LOCAL_MISSING", "本地文件不存在"));
    }
    // UTF-8 文本校验（V0.1 仅支持文本文件）
    std::fs::read_to_string(path).map_err(|_| {
        ipc_err("MAPPING_FILE_NOT_UTF8", "文件不是有效的 UTF-8 文本，V0.1 仅支持文本文件")
    })?;

    let gist_filename = if input.gist_filename.trim().is_empty() {
        path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .ok_or_else(|| ipc_err("MAPPING_FILENAME_INVALID", "无法从路径推导文件名"))?
    } else {
        input.gist_filename.trim().to_string()
    };

    let cfg = state.config.read();
    let lower = input.local_path.to_ascii_lowercase();
    for m in &cfg.mappings {
        if Some(m.id.as_str()) == exclude_id {
            continue;
        }
        if m.local_path.to_ascii_lowercase() == lower {
            return Err(ipc_err("MAPPING_LOCAL_DUPLICATE", "该本地文件已被其他映射使用"));
        }
        if m.account_id == input.account_id
            && m.gist_id == input.gist_id
            && m.gist_filename == gist_filename
        {
            return Err(ipc_err("MAPPING_TRIPLE_DUPLICATE", "该账号下相同 gist 的相同文件已有映射"));
        }
    }
    validate_hooks(&input.hooks)?;
    Ok((normalize_stored_path(&input.local_path), gist_filename))
}

/// 事件脚本配置校验（V0.6）：File 模式看文件存在 + 扩展名白名单；Inline 模式看内容大小。
/// 未配置的钩子（对应字段为空）不校验。
fn validate_hooks(hooks: &crate::model::MappingHooks) -> Result<(), String> {
    use crate::model::HookMode;
    for hook in [&hooks.pre_upload, &hooks.post_upload, &hooks.pre_download, &hooks.post_download] {
        match hook.mode {
            HookMode::File if !hook.script_path.is_empty() => {
                let p = Path::new(&hook.script_path);
                if !p.is_file() {
                    return Err(ipc_err(
                        "HOOK_SCRIPT_MISSING",
                        format!("事件脚本文件不存在: {}", hook.script_path),
                    ));
                }
                let ext_ok = p
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| matches!(e.to_ascii_lowercase().as_str(), "ps1" | "bat" | "cmd"))
                    .unwrap_or(false);
                if !ext_ok {
                    return Err(ipc_err("HOOK_SCRIPT_INVALID", "事件脚本仅支持 .ps1 / .bat / .cmd"));
                }
            }
            HookMode::Inline
                if !hook.script_content.is_empty()
                    && hook.script_content.chars().count() > 65536 =>
            {
                return Err(ipc_err("HOOK_SCRIPT_TOO_LARGE", "界面内脚本内容超过 64KB 限制"));
            }
            _ => {}
        }
    }
    Ok(())
}

#[tauri::command]
pub fn mapping_create(state: State<'_, AppState>, input: MappingInput) -> Result<Mapping, String> {
    let (local_path, gist_filename) = validate_mapping(&state, &input, None)?;
    let mapping = Mapping {
        id: uuid::Uuid::new_v4().to_string(),
        name: input.name.trim().to_string(),
        local_path,
        account_id: input.account_id,
        gist_id: input.gist_id.trim().to_string(),
        gist_filename,
        upload_enabled: input.upload_enabled,
        download_enabled: input.download_enabled,
        enabled: input.enabled,
        hooks: input.hooks,
    };
    // 唯一性校验放进 update 闭包内，消除"校验后-写入前"的并发窗口（TOCTOU）
    let dup_key = (
        mapping.account_id.clone(),
        mapping.gist_id.clone(),
        mapping.gist_filename.clone(),
    );
    let stored = mapping.clone();
    let stored_path = mapping.local_path.clone();
    let outcome = state
        .config
        .try_update::<_, String>(move |c| {
            if c
                .mappings
                .iter()
                .any(|m| m.local_path.eq_ignore_ascii_case(&stored_path))
            {
                return Err(ipc_err("MAPPING_LOCAL_DUPLICATE", "该本地文件已被其他映射使用"));
            }
            if c.mappings.iter().any(|m| {
                (m.account_id.clone(), m.gist_id.clone(), m.gist_filename.clone()) == dup_key
            }) {
                return Err(ipc_err("MAPPING_TRIPLE_DUPLICATE", "该账号下相同 gist 的相同文件已有映射"));
            }
            c.mappings.push(stored);
            Ok(())
        })
        .map_err(|e| ipc_err("CONFIG_SAVE_FAILED", e.to_string()))?;
    outcome?;
    if mapping.enabled {
        state
            .engine
            .apply_watch_set()
            .map_err(|e| ipc_err("WATCH_SET_FAILED", e.to_string()))?;
    }
    Ok(mapping)
}

#[tauri::command]
pub fn mapping_update(
    state: State<'_, AppState>,
    id: String,
    input: MappingInput,
) -> Result<(), String> {
    let (local_path, gist_filename) = validate_mapping(&state, &input, Some(&id))?;
    let dup_key = (input.account_id.clone(), input.gist_id.trim().to_string(), gist_filename.clone());
    let mut rebuild = false;
    let outcome = state
        .config
        .try_update::<_, String>(|c| {
            if c.mappings.iter().any(|other| {
                other.id != id
                    && (other.local_path.eq_ignore_ascii_case(&local_path)
                        || (other.account_id.clone(), other.gist_id.clone(), other.gist_filename.clone()) == dup_key)
            }) {
                return Err(ipc_err("MAPPING_CONFLICT", "与其他映射冲突（本地文件或 gist 文件重复）"));
            }
            let Some(m) = c.mappings.iter_mut().find(|m| m.id == id) else {
                return Err(ipc_err("MAPPING_NOT_FOUND", "映射不存在"));
            };
            rebuild = m.local_path != local_path || m.enabled != input.enabled;
            m.name = input.name.trim().to_string();
            m.local_path = local_path;
            m.account_id = input.account_id;
            m.gist_id = input.gist_id.trim().to_string();
            m.gist_filename = gist_filename;
            m.upload_enabled = input.upload_enabled;
            m.download_enabled = input.download_enabled;
            m.enabled = input.enabled;
            m.hooks = input.hooks.clone();
            Ok(())
        })
        .map_err(|e| ipc_err("CONFIG_SAVE_FAILED", e.to_string()))?;
    outcome?;
    if rebuild {
        state
            .engine
            .apply_watch_set()
            .map_err(|e| ipc_err("WATCH_SET_FAILED", e.to_string()))?;
    }
    Ok(())
}

#[tauri::command]
pub fn mapping_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .config
        .update(|c| c.mappings.retain(|m| m.id != id))
        .map_err(|e| ipc_err("CONFIG_SAVE_FAILED", e.to_string()))?;
    let _ = state.snapshots.remove_mapping(&id);
    state.engine.remove_status(&id);
    state
        .engine
        .apply_watch_set()
        .map_err(|e| ipc_err("WATCH_SET_FAILED", e.to_string()))?;
    Ok(())
}

#[tauri::command]
pub async fn mapping_set_enabled(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    state
        .config
        .update(|c| {
            if let Some(m) = c.mappings.iter_mut().find(|m| m.id == id) {
                m.enabled = enabled;
            }
        })
        .map_err(|e| ipc_err("CONFIG_SAVE_FAILED", e.to_string()))?;
    state
        .engine
        .apply_watch_set()
        .map_err(|e| ipc_err("WATCH_SET_FAILED", e.to_string()))?;
    if enabled {
        // 重开开关：立即跑一轮 Poll，补偿关闭期间错过的远端变更
        let _ = state.engine.run_one(&id, Trigger::Poll).await;
    }
    Ok(())
}

// ── gist 选择器 / 同步操作 ───────────────────────────────────────────

#[tauri::command]
pub async fn gist_list_for_account(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<Vec<GistSummary>, String> {
    let account = state
        .config
        .read()
        .accounts
        .into_iter()
        .find(|a| a.id == account_id)
        .ok_or_else(|| ipc_err("ACCOUNT_NOT_FOUND", "账号不存在"))?;
    let provider = state
        .ctx_providers()
        .get_or_build(&account)
        .await
        .map_err(|e| ipc_err("PROVIDER_REQUEST_FAILED", e.to_string()))?;
    let mut gists = provider
        .list_gists(None)
        .await
        .map_err(|e| ipc_err("PROVIDER_REQUEST_FAILED", e.to_string()))?;
    gists.sort_by_key(|g| std::cmp::Reverse(g.updated_at));
    Ok(gists)
}

#[tauri::command]
pub async fn sync_manual(
    state: State<'_, AppState>,
    mapping_id: String,
    direction: String,
) -> Result<MappingStatus, String> {
    let trigger = match direction.as_str() {
        "upload" => Trigger::ManualUpload,
        "download" => Trigger::ManualDownload,
        other => return Err(ipc_err("SYNC_DIRECTION_INVALID", format!("未知的同步方向: {other}"))),
    };
    Ok(state.engine.run_one(&mapping_id, trigger).await)
}

#[tauri::command]
pub fn sync_set_paused(state: State<'_, AppState>, paused: bool) {
    state.engine.set_global_paused(paused);
}

#[tauri::command]
pub fn sync_is_paused(state: State<'_, AppState>) -> bool {
    state.engine.is_global_paused()
}

/// 同步操作流水查询（新在前；limit=None 全量，V0.5 日志页用）
#[tauri::command]
pub fn log_list(state: State<'_, AppState>, limit: Option<usize>) -> Vec<crate::synclog::SyncLogEntry> {
    state.synclog.tail(limit)
}

/// 回滚到本地归档版本（本地与远端同时恢复）
#[tauri::command]
pub async fn history_rollback_local(
    state: State<'_, AppState>,
    mapping_id: String,
    file_name: String,
) -> Result<MappingStatus, String> {
    if !crate::sync::history::is_archive_name(&file_name) {
        return Err(ipc_err("HISTORY_FILE_INVALID", "非法的归档文件名"));
    }
    let account_id = {
        let cfg = state.config.read();
        let Some(m) = cfg.mappings.iter().find(|m| m.id == mapping_id) else {
            return Err(ipc_err("MAPPING_NOT_FOUND", "映射不存在"));
        };
        m.account_id.clone()
    };
    if !state.config.read().accounts.iter().any(|a| a.id == account_id) {
        return Err(ipc_err("ACCOUNT_NOT_FOUND", "账号不存在"));
    }
    Ok(state.engine.rollback_to_archive(&mapping_id, &file_name).await)
}

/// 回滚到远端 gist 指定版本（本地与远端同时恢复）
#[tauri::command]
pub async fn history_rollback_remote(
    state: State<'_, AppState>,
    mapping_id: String,
    revision: String,
) -> Result<MappingStatus, String> {
    let account_id = {
        let cfg = state.config.read();
        let Some(m) = cfg.mappings.iter().find(|m| m.id == mapping_id) else {
            return Err(ipc_err("MAPPING_NOT_FOUND", "映射不存在"));
        };
        m.account_id.clone()
    };
    if !state.config.read().accounts.iter().any(|a| a.id == account_id) {
        return Err(ipc_err("ACCOUNT_NOT_FOUND", "账号不存在"));
    }
    Ok(state.engine.rollback_to_revision(&mapping_id, &revision).await)
}

// ── 全局设置 ─────────────────────────────────────────────────────────

#[tauri::command]
pub fn settings_get(state: State<'_, AppState>) -> GlobalSettings {
    state.config.read().settings
}

#[tauri::command]
pub fn settings_update(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: GlobalSettings,
) -> Result<(), String> {
    if !(1..=3600).contains(&settings.debounce_seconds) {
        return Err(ipc_err("SETTING_DEBOUNCE_RANGE", "防抖秒数须在 1~3600 之间"));
    }
    if !(1..=1440).contains(&settings.poll_interval_minutes) {
        return Err(ipc_err("SETTING_POLL_RANGE", "轮询间隔须在 1~1440 分钟之间"));
    }
    if !["info", "warn", "error"].contains(&settings.log_level.as_str()) {
        return Err(ipc_err("SETTING_LOG_LEVEL_INVALID", "日志级别仅支持 info/warn/error"));
    }
    if !["zh-CN", "en"].contains(&settings.language.as_str()) {
        return Err(ipc_err("SETTING_LANGUAGE_INVALID", "语言仅支持 zh-CN/en"));
    }
    let debounce_changed = state.config.read().settings.debounce_seconds != settings.debounce_seconds;
    let debounce_secs = settings.debounce_seconds;
    let auto_start_changed = state.config.read().settings.auto_start != settings.auto_start;
    let auto_start_target = settings.auto_start;
    let log_level_changed = state.config.read().settings.log_level != settings.log_level;
    let log_level = settings.log_level.clone();
    // 副作用先行：开机自启注册/注销失败时配置尚未落盘，两边保持一致
    if auto_start_changed {
        use tauri_plugin_autostart::ManagerExt;
        let autolaunch = app.autolaunch();
        let result = if auto_start_target {
            autolaunch.enable()
        } else {
            autolaunch.disable()
        };
        result.map_err(|e| ipc_err("AUTOSTART_UPDATE_FAILED", format!("更新开机自启设置失败: {e}")))?;
    }
    if let Err(e) = state.config.update(|c| c.settings = settings) {
        // 配置落盘失败：回滚刚执行的副作用（best-effort），避免注册表与磁盘不一致
        if auto_start_changed {
            use tauri_plugin_autostart::ManagerExt;
            let autolaunch = app.autolaunch();
            let _ = if auto_start_target {
                autolaunch.disable()
            } else {
                autolaunch.enable()
            };
        }
        return Err(ipc_err("CONFIG_SAVE_FAILED", e.to_string()));
    }
    if debounce_changed {
        // 防抖管道常驻且动态读窗口：只更新原子值，不重建 watcher
        state.engine.set_debounce_window(debounce_secs);
    }
    // 日志级别热更新：失败不回滚配置（级别已保存，重启后生效），仅告警留痕
    if log_level_changed
        && let Some(handle) = &state.log_reload
        && let Err(e) = crate::logging::set_log_level(handle, &log_level)
    {
        tracing::warn!("日志级别热更新失败（重启后生效）: {e}");
    }
    Ok(())
}

// ── 其他 ─────────────────────────────────────────────────────────────

/// 冲突弹窗预览：本地当前内容 vs 远端当前内容
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictPreview {
    pub local: Option<String>,
    pub remote: Option<String>,
}

#[tauri::command]
pub async fn conflict_preview(
    state: State<'_, AppState>,
    mapping_id: String,
) -> Result<ConflictPreview, String> {
    let mapping = state
        .config
        .read()
        .mappings
        .into_iter()
        .find(|m| m.id == mapping_id)
        .ok_or_else(|| ipc_err("MAPPING_NOT_FOUND", "映射不存在"))?;
    let account = state
        .config
        .read()
        .accounts
        .into_iter()
        .find(|a| a.id == mapping.account_id)
        .ok_or_else(|| ipc_err("ACCOUNT_NOT_FOUND", "账号不存在"))?;
    let local = std::fs::read_to_string(&mapping.local_path).ok();
    let provider = state
        .ctx_providers()
        .get_or_build(&account)
        .await
        .map_err(|e| ipc_err("PROVIDER_REQUEST_FAILED", e.to_string()))?;
    let detail = provider
        .get_gist(&mapping.gist_id, Some(&mapping.gist_filename))
        .await
        .map_err(|e| ipc_err("PROVIDER_REQUEST_FAILED", e.to_string()))?;
    let remote = detail
        .files
        .into_iter()
        .find(|f| f.filename == mapping.gist_filename)
        .and_then(|f| f.content);
    Ok(ConflictPreview { local, remote })
}

#[tauri::command]
pub fn app_open_log_dir(state: State<'_, AppState>) -> Result<(), String> {
    std::fs::create_dir_all(&state.log_dir)
        .map_err(|e| ipc_err("LOG_DIR_OPEN_FAILED", format!("创建日志目录失败: {e}")))?;
    crate::open_in_explorer(&state.log_dir)
        .map_err(|e| ipc_err("LOG_DIR_OPEN_FAILED", e))
}

impl AppState {
    /// 引擎持有的 provider 缓存（账号 token/实例变更后失效）
    fn ctx_providers(&self) -> &ProviderCache {
        &self.engine.ctx.providers
    }
}

// ── 历史与版本查询（V0.3）──────────────────────────────────────────────

/// 对比预览：历史内容 + 当前本地内容（对比弹窗一次取全，形态同 conflict_preview）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPreview {
    pub historical: String,
    pub current_local: String,
}

/// 归档文件名安全校验（防路径穿越）：不合法 → HISTORY_FILE_INVALID
fn ensure_archive_name(name: &str) -> Result<(), String> {
    if crate::sync::history::is_archive_name(name) {
        Ok(())
    } else {
        Err(ipc_err("HISTORY_FILE_INVALID", "非法的归档文件名"))
    }
}

#[tauri::command]
pub fn history_list(
    state: State<'_, AppState>,
    mapping_id: String,
) -> Result<Vec<HistoryEntry>, String> {
    let has = state.config.read().mappings.iter().any(|m| m.id == mapping_id);
    if !has {
        return Err(ipc_err("MAPPING_NOT_FOUND", "映射不存在"));
    }
    Ok(state.snapshots.history_list(&mapping_id))
}

#[tauri::command]
pub async fn history_preview(
    state: State<'_, AppState>,
    mapping_id: String,
    file_name: String,
) -> Result<HistoryPreview, String> {
    ensure_archive_name(&file_name)?;
    let mapping = state
        .config
        .read()
        .mappings
        .into_iter()
        .find(|m| m.id == mapping_id)
        .ok_or_else(|| ipc_err("MAPPING_NOT_FOUND", "映射不存在"))?;
    let historical = state.snapshots.read_archive(&mapping_id, &file_name).map_err(|e| {
        match e.kind() {
            std::io::ErrorKind::InvalidInput => ipc_err("HISTORY_FILE_INVALID", "非法的归档文件名"),
            std::io::ErrorKind::NotFound => ipc_err("HISTORY_FILE_NOT_FOUND", "归档文件不存在"),
            _ => ipc_err("HISTORY_READ_FAILED", format!("读取归档失败: {e}")),
        }
    })?;
    // 本地文件不存在（Missing 态）→ 空串，对比弹窗该栏为空不报错（spec §6 边界）
    let current_local = std::fs::read_to_string(&mapping.local_path).unwrap_or_default();
    Ok(HistoryPreview { historical, current_local })
}

#[tauri::command]
pub async fn gist_commit_list(
    state: State<'_, AppState>,
    mapping_id: String,
) -> Result<Vec<GistCommit>, String> {
    let mapping = state
        .config
        .read()
        .mappings
        .into_iter()
        .find(|m| m.id == mapping_id)
        .ok_or_else(|| ipc_err("MAPPING_NOT_FOUND", "映射不存在"))?;
    let account = state
        .config
        .read()
        .accounts
        .into_iter()
        .find(|a| a.id == mapping.account_id)
        .ok_or_else(|| ipc_err("ACCOUNT_NOT_FOUND", "账号不存在"))?;
    let provider = state
        .ctx_providers()
        .get_or_build(&account)
        .await
        .map_err(|e| ipc_err("PROVIDER_REQUEST_FAILED", e.to_string()))?;
    provider
        .list_commits(&mapping.gist_id)
        .await
        .map_err(|e| ipc_err("PROVIDER_REQUEST_FAILED", e.to_string()))
}

#[tauri::command]
pub async fn gist_commit_preview(
    state: State<'_, AppState>,
    mapping_id: String,
    revision: String,
) -> Result<HistoryPreview, String> {
    let mapping = state
        .config
        .read()
        .mappings
        .into_iter()
        .find(|m| m.id == mapping_id)
        .ok_or_else(|| ipc_err("MAPPING_NOT_FOUND", "映射不存在"))?;
    let account = state
        .config
        .read()
        .accounts
        .into_iter()
        .find(|a| a.id == mapping.account_id)
        .ok_or_else(|| ipc_err("ACCOUNT_NOT_FOUND", "账号不存在"))?;
    let provider = state
        .ctx_providers()
        .get_or_build(&account)
        .await
        .map_err(|e| ipc_err("PROVIDER_REQUEST_FAILED", e.to_string()))?;
    let historical = provider
        .get_file_at_revision(&mapping.gist_id, &revision, &mapping.gist_filename)
        .await
        .map_err(|e| ipc_err("PROVIDER_REQUEST_FAILED", e.to_string()))?;
    let current_local = std::fs::read_to_string(&mapping.local_path).unwrap_or_default();
    Ok(HistoryPreview { historical, current_local })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 终审修复 #1：正斜杠手填路径归一为反斜杠，watcher 才能匹配 notify 事件
    #[test]
    fn normalize_stored_path_unifies_separators() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.md");
        std::fs::write(&file, "x").unwrap();
        let raw = file.to_string_lossy().replace('\\', "/");
        let normalized = normalize_stored_path(&raw);
        assert!(normalized.contains('\\'), "应统一为反斜杠: {normalized}");
        assert!(!normalized.contains("\\\\?\\"), "不应保留 verbatim 前缀: {normalized}");
        assert!(normalize_stored_path(&normalized).eq_ignore_ascii_case(&normalized), "幂等");
    }

    #[test]
    fn normalize_stored_path_missing_file_falls_back_to_separator_fix() {
        assert_eq!(
            normalize_stored_path("D:/ghost dir/f.md"),
            "D:\\ghost dir\\f.md"
        );
    }

    /// V0.3：归档文件名安全校验（防路径穿越），命令层错误码 HISTORY_FILE_INVALID
    #[test]
    fn ensure_archive_name_rejects_traversal_and_accepts_valid() {
        for bad in ["..\\config.json", "a/b-local.md", "xx-local.md"] {
            let err = ensure_archive_name(bad).unwrap_err();
            assert!(err.contains("HISTORY_FILE_INVALID"), "bad={bad}, err={err}");
        }
        assert!(ensure_archive_name("20260101000000000-local.md").is_ok());
        assert!(ensure_archive_name("20260101000000000-remote.txt").is_ok());
    }

    // ── 事件脚本校验（V0.6）────────────────────────────────────────

    fn hook_with_path(path: &str) -> crate::model::EventHook {
        crate::model::EventHook {
            script_path: path.into(),
            ..crate::model::EventHook::default()
        }
    }

    #[test]
    fn file_hook_validation_matrix() {
        let dir = tempfile::tempdir().unwrap();
        let ps1 = dir.path().join("a.ps1");
        std::fs::write(&ps1, "exit 0").unwrap();
        let ps1_upper = dir.path().join("B.PS1");
        std::fs::write(&ps1_upper, "exit 0").unwrap();
        let exe = dir.path().join("c.exe");
        std::fs::write(&exe, "x").unwrap();

        let mut hooks = crate::model::MappingHooks::default();
        assert!(validate_hooks(&hooks).is_ok(), "全空 hooks 应通过");

        hooks.pre_upload = hook_with_path(&ps1.to_string_lossy());
        assert!(validate_hooks(&hooks).is_ok(), "存在的 ps1 应通过");

        hooks.post_download = hook_with_path(&ps1_upper.to_string_lossy());
        assert!(validate_hooks(&hooks).is_ok(), "大写 .PS1 应通过（扩展名不区分大小写）");

        hooks.post_upload = hook_with_path(&dir.path().join("not-exist.ps1").to_string_lossy());
        let err = validate_hooks(&hooks).unwrap_err();
        assert!(err.contains("HOOK_SCRIPT_MISSING"), "err={err}");

        hooks.post_upload = hook_with_path(&exe.to_string_lossy());
        let err = validate_hooks(&hooks).unwrap_err();
        assert!(err.contains("HOOK_SCRIPT_INVALID"), "err={err}");
    }

    #[test]
    fn inline_size_boundary_65536_ok_65537_rejected() {
        let mut hooks = crate::model::MappingHooks::default();
        hooks.pre_download = crate::model::EventHook {
            mode: crate::model::HookMode::Inline,
            script_content: "a".repeat(65536),
            ..crate::model::EventHook::default()
        };
        assert!(validate_hooks(&hooks).is_ok(), "恰好 65536 字符应通过");
        hooks.pre_download.script_content = "a".repeat(65537);
        let err = validate_hooks(&hooks).unwrap_err();
        assert!(err.contains("HOOK_SCRIPT_TOO_LARGE"), "err={err}");
    }

    #[test]
    fn mapping_input_legacy_without_hooks_deserializes() {
        let input: MappingInput = serde_json::from_str(
            r#"{
            "name": "", "localPath": "C:/a.md", "accountId": "a", "gistId": "g",
            "gistFilename": "a.md", "uploadEnabled": true, "downloadEnabled": true, "enabled": true
        }"#,
        )
        .unwrap();
        assert!(!input.hooks.pre_upload.is_configured());
    }
}
