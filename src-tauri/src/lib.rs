pub mod commands;
pub mod logging;
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
        // 单实例：必须最先注册；二次启动唤起主窗口
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
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
            let log_level = config.read().settings.log_level.clone();
            let log_reload = match logging::init_logging(&log_dir, &log_level) {
                Ok(h) => Some(h),
                // 日志初始化失败不阻塞应用，但要在控制台可见；级别改动退化为重启生效
                Err(e) => {
                    eprintln!("{e}");
                    None
                }
            };
            // panic=abort 下闪退无痕：hook 同步直写 panic.log，成败均安装
            logging::install_panic_hook(&log_dir);

            // 开机自启按设置显式对齐一次：首次安装（默认 autoStart=true）也要注册成功
            {
                use tauri_plugin_autostart::ManagerExt;
                let autolaunch = handle.autolaunch();
                let want = config.read().settings.auto_start;
                let result = if want {
                    autolaunch.enable()
                } else {
                    autolaunch.disable()
                };
                if let Err(e) = result {
                    tracing::warn!("对齐开机自启设置失败: {e}");
                }
            }

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
                log_reload,
            });

            setup_tray(app)?;

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
        // 关窗 = 隐藏到托盘；退出走托盘菜单
        .on_window_event(|window, event| {
            if window.label() == "main"
                && let tauri::WindowEvent::CloseRequested { api, .. } = event
            {
                api.prevent_close();
                let _ = window.hide();
            }
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
            commands::history_rollback_local,
            commands::history_rollback_remote,
            commands::history_list,
            commands::history_preview,
            commands::gist_commit_list,
            commands::gist_commit_preview,
            commands::settings_get,
            commands::settings_update,
            commands::conflict_preview,
            commands::app_open_log_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// 显示（并聚焦）主窗口——托盘菜单/左键点击/二次启动共用
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// 托盘：菜单（打开主窗口/退出）+ 左键点击唤起
fn setup_tray(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let show = MenuItem::with_id(app, "show", "打开主窗口", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    let icon = app
        .default_window_icon()
        .ok_or("缺少默认窗口图标，无法创建托盘")?
        .clone();

    let tray = TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .tooltip("Gist Auto Sync")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    // 托盘图标需与应用同生命周期
    app.manage(tray);
    Ok(())
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
