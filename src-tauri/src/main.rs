// Prevents an additional console window on Windows (both debug and release).
#![windows_subsystem = "windows"]

mod changelog;
mod launcher;
mod menu;
mod settings;

#[cfg(windows)]
mod job_object;

use tauri::Manager;
use tauri_plugin_updater::UpdaterExt;

/// Check for updates via the configured updater endpoints.
/// Returns `Some(new_version)` if an update is available, `None` otherwise.
#[tauri::command]
async fn check_update(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let updater = app
        .updater()
        .map_err(|e| format!("updater init failed: {e}"))?;
    match updater.check().await {
        Ok(Some(update)) => Ok(Some(update.version)),
        Ok(None) => Ok(None),
        Err(e) => Err(format!("check failed: {e}")),
    }
}

/// Download and install the latest update. This launches the platform installer
/// (NSIS on Windows), which replaces the app and relaunches it.
#[tauri::command]
async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    let updater = app
        .updater()
        .map_err(|e| format!("updater init failed: {e}"))?;
    let update = updater
        .check()
        .await
        .map_err(|e| format!("check failed: {e}"))?
        .ok_or_else(|| "no update available".to_string())?;

    update
        .download_and_install(
            |_chunk_length, _content_length| {
                // Progress is not surfaced per-chunk here; the frontend shows a
                // generic "downloading" message.
            },
            || {
                // Called when download finishes (before install).
            },
        )
        .await
        .map_err(|e| format!("download/install failed: {e}"))?;

    Ok(())
}

/// Return the desktop app version (from the package info).
#[tauri::command]
fn get_desktop_version(app: tauri::AppHandle) -> String {
    app.package_info().version.to_string()
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            launcher::start_dsh,
            launcher::get_dsh_url,
            launcher::get_log_history,
            launcher::get_dsh_version,
            launcher::list_plugins,
            launcher::install_plugin,
            launcher::remove_plugin,
            launcher::export_config,
            launcher::import_config,
            check_update,
            install_update,
            get_desktop_version,
            settings::get_settings,
            settings::update_settings,
            changelog::get_changelog
        ])
        .setup(|app| {
            // Build the native menu and attach it ONLY to the main window.
            // Loading/tools windows get no menu bar.
            let handle = app.handle().clone();
            match menu::build_menu(&handle) {
                Ok(m) => {
                    if let Some(main_win) = app.get_webview_window("main") {
                        let _ = main_win.set_menu(m);
                    }
                }
                Err(e) => {
                    eprintln!("failed to build menu: {e}");
                }
            }

            // Handle menu events (app-level; fires for the main window menu).
            app.on_menu_event(|app, event| {
                let id = event.id().0.as_str().to_string();
                menu::handle_menu_event(app, &id);
            });

            // NOTE: DSH launch is NOT started here. The frontend drives the
            // flow: it first checks for updates (auto-installs if found), then
            // calls `start_dsh` only when no update is pending. This avoids the
            // previous race where the backend launched DSH before the frontend
            // was ready to receive logs.
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, event| {
            match event {
                tauri::RunEvent::ExitRequested { .. } => {
                    // Ensure the DSH child process is terminated when the app exits.
                    launcher::kill_dsh();
                }
                tauri::RunEvent::WindowEvent { label, event, .. } => {
                    // Kill DSH when the main window is closed/destroyed, so the
                    // child process tree never leaks (even if the app keeps
                    // running with other windows open).
                    if label == "main" {
                        match event {
                            tauri::WindowEvent::CloseRequested { .. }
                            | tauri::WindowEvent::Destroyed => {
                                launcher::kill_dsh();
                            }
                            _ => {}
                        }
                    }
                }
                tauri::RunEvent::Exit => {
                    // Final fallback: ensure DSH is gone on any exit path.
                    launcher::kill_dsh();
                }
                _ => {}
            }
        });
}
