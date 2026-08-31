// Prevents an additional console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod launcher;

use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![launcher::start_dsh, launcher::get_dsh_url])
        .setup(|app| {
            // Spawn the DSH launch flow as soon as the loading window is ready.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                // Give the loading window a moment to render, then start DSH.
                std::thread::sleep(std::time::Duration::from_millis(300));
                let _ = launcher::launch_dsh(&handle);
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                // Ensure the DSH child process is terminated when the app exits.
                launcher::kill_dsh();
            }
        });
}
