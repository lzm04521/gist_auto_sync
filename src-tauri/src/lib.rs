pub mod commands;
pub mod config;
pub mod model;
pub mod provider;
pub mod secrets;
pub mod sync;

use std::sync::Arc;

use tauri::Manager;

use config::ConfigStore;
use sync::engine::{ProviderCache, SyncCtx};
use sync::service::Engine;
use sync::snapshot::{SelfWriteGuard, SnapshotStore};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let config_dir = handle
                .path()
                .app_config_dir()
                .expect("无法确定应用配置目录");
            std::fs::create_dir_all(&config_dir)
                .map_err(|e| format!("创建配置目录失败: {e}"))?;

            let config = Arc::new(
                ConfigStore::load_from(config_dir.join("config.json"))
                    .map_err(|e| format!("加载配置失败: {e}"))?,
            );
            let snapshots = Arc::new(SnapshotStore::new(config_dir.join("snapshots")));
            let log_dir = handle.path().app_log_dir().expect("无法确定日志目录");

            let sink: Arc<dyn sync::engine::StatusSink> =
                Arc::new(commands::IpcSink { app: handle.clone() });
            let ctx = Arc::new(SyncCtx {
                config: config.clone(),
                snapshots: snapshots.clone(),
                providers: ProviderCache::new(),
                guard: SelfWriteGuard::new(),
                sink,
            });
            let engine = Arc::new(
                Engine::new(ctx, config_dir.join("statuses.json"))
                    .map_err(|e| format!("初始化同步引擎失败: {e}"))?,
            );

            app.manage(commands::AppState {
                config,
                snapshots,
                engine: engine.clone(),
                log_dir,
            });

            // 引擎常驻任务 + 启动对账
            tauri::async_runtime::spawn(async move {
                if let Err(e) = engine.start() {
                    tracing::error!("同步引擎启动失败: {e}");
                    return;
                }
                engine.startup_reconcile().await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::account_list,
            commands::account_create,
            commands::account_update,
            commands::account_delete,
            commands::account_test,
            commands::mapping_list,
            commands::mapping_create,
            commands::mapping_update,
            commands::mapping_delete,
            commands::mapping_set_enabled,
            commands::gist_list_for_account,
            commands::sync_manual,
            commands::sync_set_paused,
            commands::sync_is_paused,
            commands::settings_get,
            commands::settings_update,
            commands::app_open_log_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// 在资源管理器中打开目录（opener 插件）
pub fn open_in_explorer(dir: &std::path::Path) -> Result<(), String> {
    tauri_plugin_opener::open_path(dir.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn scaffold_ok() {
        assert_eq!(1 + 1, 2);
    }
}
