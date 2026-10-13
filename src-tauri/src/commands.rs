use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::config::ConfigStore;
use crate::model::{Account, GlobalSettings, Mapping, ProviderKind};
use crate::provider::{GistSummary, UserInfo};
use crate::secrets;
use crate::sync::engine::{MappingStatus, ProviderCache, StatusEvent, StatusSink, Trigger};
use crate::sync::service::{Engine, PersistedStatus};
use crate::sync::snapshot::SnapshotStore;

/// IPC 全局状态（setup 时构造并 manage）
pub struct AppState {
    pub config: Arc<ConfigStore>,
    pub snapshots: Arc<SnapshotStore>,
    pub engine: Arc<Engine>,
    pub log_dir: std::path::PathBuf,
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
    pub local_path: String,
    pub account_id: String,
    pub gist_id: String,
    /// 留空默认取本地文件名
    pub gist_filename: String,
    pub upload_enabled: bool,
    pub download_enabled: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
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
    let id = uuid::Uuid::new_v4().to_string();
    let token_ref = secrets::token_ref_for(&id);
    secrets::save_token(&token_ref, &input.token).map_err(|e| e.to_string())?;

    let base_url = match input.provider {
        ProviderKind::Github => "https://api.github.com".to_string(),
        ProviderKind::Opengist => {
            let url = input.base_url.trim().trim_end_matches('/').to_string();
            if !url.starts_with("http") {
                return Err("Opengist 实例地址必须以 http(s):// 开头".into());
            }
            url
        }
    };
    let account = Account {
        id,
        name: input.name.trim().to_string(),
        provider: input.provider,
        base_url,
        token_ref,
    };
    state
        .config
        .update(|c| c.accounts.push(account.clone()))
        .map_err(|e| e.to_string())?;
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
        .ok_or("账号不存在")?;
    let token_ref = existing.token_ref.clone();
    if !input.token.trim().is_empty() {
        secrets::save_token(&token_ref, input.token.trim()).map_err(|e| e.to_string())?;
    }
    let base_url = match input.provider {
        ProviderKind::Github => "https://api.github.com".to_string(),
        ProviderKind::Opengist => {
            let url = input.base_url.trim().trim_end_matches('/').to_string();
            if !url.starts_with("http") {
                return Err("Opengist 实例地址必须以 http(s):// 开头".into());
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
        .map_err(|e| e.to_string())?;
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
        .ok_or("账号不存在")?;
    let mappings: Vec<Mapping> = state
        .config
        .read()
        .mappings
        .into_iter()
        .filter(|m| m.account_id == id)
        .collect();
    if !mappings.is_empty() && !force {
        return Err(format!(
            "该账号还有 {} 条同步映射；删除映射后再试，或选择强制删除（将一并删除映射与其快照）",
            mappings.len()
        ));
    }
    for m in &mappings {
        let _ = state.snapshots.remove_mapping(&m.id);
    }
    state
        .config
        .update(|c| {
            c.accounts.retain(|a| a.id != id);
            c.mappings.retain(|m| m.account_id != id);
        })
        .map_err(|e| e.to_string())?;
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
        .ok_or("账号不存在")?;
    let provider = state
        .ctx_providers()
        .get_or_build(&account)
        .await
        .map_err(|e| e.to_string())?;
    provider.verify_token().await.map_err(|e| e.to_string())
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

/// 映射输入校验：文件存在、UTF-8、localPath 唯一（大小写不敏感）、(账号,gist,文件名) 唯一
fn validate_mapping(
    state: &AppState,
    input: &MappingInput,
    exclude_id: Option<&str>,
) -> Result<(String, String), String> {
    if !state
        .config
        .read()
        .accounts
        .iter()
        .any(|a| a.id == input.account_id)
    {
        return Err("所选账号不存在".into());
    }
    let path = Path::new(&input.local_path);
    if !path.is_absolute() {
        return Err("本地路径必须是绝对路径".into());
    }
    if !path.is_file() {
        return Err("本地文件不存在".into());
    }
    // UTF-8 文本校验（V0.1 仅支持文本文件）
    std::fs::read_to_string(path).map_err(|_| "文件不是有效的 UTF-8 文本，V0.1 仅支持文本文件")?;

    let gist_filename = if input.gist_filename.trim().is_empty() {
        path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .ok_or("无法从路径推导文件名")?
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
            return Err("该本地文件已被其他映射使用".into());
        }
        if m.account_id == input.account_id
            && m.gist_id == input.gist_id
            && m.gist_filename == gist_filename
        {
            return Err("该账号下相同 gist 的相同文件已有映射".into());
        }
    }
    Ok((input.local_path.clone(), gist_filename))
}

#[tauri::command]
pub fn mapping_create(state: State<'_, AppState>, input: MappingInput) -> Result<Mapping, String> {
    let (local_path, gist_filename) = validate_mapping(&state, &input, None)?;
    let mapping = Mapping {
        id: uuid::Uuid::new_v4().to_string(),
        local_path,
        account_id: input.account_id,
        gist_id: input.gist_id.trim().to_string(),
        gist_filename,
        upload_enabled: input.upload_enabled,
        download_enabled: input.download_enabled,
        enabled: input.enabled,
    };
    state
        .config
        .update(|c| c.mappings.push(mapping.clone()))
        .map_err(|e| e.to_string())?;
    if mapping.enabled {
        state.engine.apply_watch_set().map_err(|e| e.to_string())?;
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
    let mut rebuild = false;
    state
        .config
        .update(|c| {
            if let Some(m) = c.mappings.iter_mut().find(|m| m.id == id) {
                rebuild = m.local_path != local_path || m.enabled != input.enabled;
                m.local_path = local_path;
                m.account_id = input.account_id;
                m.gist_id = input.gist_id.trim().to_string();
                m.gist_filename = gist_filename;
                m.upload_enabled = input.upload_enabled;
                m.download_enabled = input.download_enabled;
                m.enabled = input.enabled;
            }
        })
        .map_err(|e| e.to_string())?;
    if rebuild {
        state.engine.apply_watch_set().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn mapping_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .config
        .update(|c| c.mappings.retain(|m| m.id != id))
        .map_err(|e| e.to_string())?;
    let _ = state.snapshots.remove_mapping(&id);
    state.engine.apply_watch_set().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn mapping_set_enabled(state: State<'_, AppState>, id: String, enabled: bool) -> Result<(), String> {
    state
        .config
        .update(|c| {
            if let Some(m) = c.mappings.iter_mut().find(|m| m.id == id) {
                m.enabled = enabled;
            }
        })
        .map_err(|e| e.to_string())?;
    state.engine.apply_watch_set().map_err(|e| e.to_string())?;
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
        .ok_or("账号不存在")?;
    let provider = state
        .ctx_providers()
        .get_or_build(&account)
        .await
        .map_err(|e| e.to_string())?;
    let mut gists = provider.list_gists(None).await.map_err(|e| e.to_string())?;
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
        other => return Err(format!("未知的同步方向: {other}")),
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
        return Err("防抖秒数须在 1~3600 之间".into());
    }
    if !(1..=1440).contains(&settings.poll_interval_minutes) {
        return Err("轮询间隔须在 1~1440 分钟之间".into());
    }
    if !["info", "warn", "error"].contains(&settings.log_level.as_str()) {
        return Err("日志级别仅支持 info/warn/error".into());
    }
    if !["zh-CN", "en"].contains(&settings.language.as_str()) {
        return Err("语言仅支持 zh-CN/en".into());
    }
    let debounce_changed = state.config.read().settings.debounce_seconds != settings.debounce_seconds;
    let auto_start_changed = state.config.read().settings.auto_start != settings.auto_start;
    let auto_start_target = settings.auto_start;
    state
        .config
        .update(|c| c.settings = settings)
        .map_err(|e| e.to_string())?;
    if debounce_changed {
        state.engine.apply_watch_set().map_err(|e| e.to_string())?;
    }
    // 开机自启联动 autostart 插件
    if auto_start_changed {
        use tauri_plugin_autostart::ManagerExt;
        let autolaunch = app.autolaunch();
        let result = if auto_start_target {
            autolaunch.enable()
        } else {
            autolaunch.disable()
        };
        result.map_err(|e| format!("更新开机自启设置失败: {e}"))?;
    }
    Ok(())
}

// ── 其他 ─────────────────────────────────────────────────────────────

#[tauri::command]
pub fn app_open_log_dir(state: State<'_, AppState>) -> Result<(), String> {
    std::fs::create_dir_all(&state.log_dir).map_err(|e| e.to_string())?;
    crate::open_in_explorer(&state.log_dir)
}

impl AppState {
    /// 引擎持有的 provider 缓存（账号 token/实例变更后失效）
    fn ctx_providers(&self) -> &ProviderCache {
        &self.engine.ctx.providers
    }
}
