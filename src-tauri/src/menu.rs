//! Native application menu: "运行" (install plugin), "查看" (list plugins),
//! "设置" (launcher / version channel), and "关于" (desktop version / DSH
//! version / check update / changelog).
//!
//! Menu clicks open a dedicated HTML "tools" window and tell it which page to
//! show. The tools window performs the actual work via Tauri commands.

use std::sync::Mutex;

use tauri::menu::{Menu, MenuBuilder, MenuItem, MenuItemBuilder, SubmenuBuilder};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

const MENU_RUN_INSTALL_PLUGIN: &str = "run_install_plugin";
const MENU_RUN_EXPORT_CONFIG: &str = "run_export_config";
const MENU_RUN_IMPORT_CONFIG: &str = "run_import_config";
const MENU_VIEW_LIST_PLUGINS: &str = "view_list_plugins";
const MENU_VIEW_DEVTOOLS: &str = "view_devtools";
const MENU_SETTINGS: &str = "settings";
const MENU_ABOUT_DSH_VERSION: &str = "about_dsh_version";
const MENU_ABOUT_DESKTOP: &str = "about_desktop";
const MENU_ABOUT_CHANGELOG: &str = "about_changelog";
/// Menu item that lights up when a new desktop version is available.
const MENU_ABOUT_UPDATE_AVAILABLE: &str = "about_update_available";
/// Tray "退出" item: kills DSH and exits the whole app.
const MENU_QUIT: &str = "tray_quit";

/// Handle to the "update available" menu item, so the async update check can
/// enable it and rewrite its label once a newer version is found.
static UPDATE_AVAILABLE_ITEM: Mutex<Option<MenuItem<tauri::Wry>>> = Mutex::new(None);

/// Keeps the tray icon alive for the app's lifetime (dropping the handle would
/// remove the tray icon).
static TRAY: Mutex<Option<tauri::tray::TrayIcon>> = Mutex::new(None);

/// Build the native app menu.
pub fn build_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let run_submenu = SubmenuBuilder::new(app, "运行")
        .item(
            &MenuItemBuilder::with_id(MENU_RUN_INSTALL_PLUGIN, "安装插件…")
                .build(app)?,
        )
        .separator()
        .item(
            &MenuItemBuilder::with_id(MENU_RUN_EXPORT_CONFIG, "导出配置…")
                .build(app)?,
        )
        .item(
            &MenuItemBuilder::with_id(MENU_RUN_IMPORT_CONFIG, "导入配置…")
                .build(app)?,
        )
        .build()?;

    let view_submenu = SubmenuBuilder::new(app, "查看")
        .item(
            &MenuItemBuilder::with_id(MENU_VIEW_LIST_PLUGINS, "已安装插件")
                .build(app)?,
        )
        .separator()
        .item(
            &MenuItemBuilder::with_id(MENU_VIEW_DEVTOOLS, "调试工具")
                .build(app)?,
        )
        .build()?;

    // "设置" is a single menu item inside the 关于 group (not its own submenu).
    let settings_item = MenuItemBuilder::with_id(MENU_SETTINGS, "设置…").build(app)?;

    let about_submenu = SubmenuBuilder::new(app, "关于")
        .item(&settings_item)
        .separator()
        .item(
            &MenuItemBuilder::with_id(MENU_ABOUT_CHANGELOG, "更新日志…")
                .build(app)?,
        )
        .item(
            &MenuItemBuilder::with_id(MENU_ABOUT_DSH_VERSION, "DeepSeek Harness 版本")
                .build(app)?,
        )
        .item(
            &MenuItemBuilder::with_id(MENU_ABOUT_DESKTOP, "关于 DeepSeek Harness Desktop")
                .build(app)?,
        )
        .build()?;

    // The "update available" indicator is a standalone top-level item at the
    // far right of the menu bar (not inside 关于). Created disabled; it is
    // enabled and relabeled by the async update check when a new version is
    // found.
    let update_item = MenuItemBuilder::with_id(MENU_ABOUT_UPDATE_AVAILABLE, "正在检查新版本…")
        .enabled(false)
        .build(app)?;
    *UPDATE_AVAILABLE_ITEM.lock().unwrap() = Some(update_item.clone());

    let menu = MenuBuilder::new(app)
        .items(&[&run_submenu, &view_submenu, &about_submenu, &update_item])
        .build()?;

    Ok(menu)
}

/// Set the "update available" menu indicator: enable it (and relabel it) when a
/// new version is found, or hide/disable it otherwise.
///
/// Called by the async update check once it knows whether an update exists.
pub fn set_update_available(latest: Option<String>) {
    let guard = UPDATE_AVAILABLE_ITEM.lock().unwrap();
    if let Some(item) = guard.as_ref() {
        match latest {
            Some(v) => {
                let _ = item.set_text(&format!("发现新版本 v{v} — 点击更新"));
                let _ = item.set_enabled(true);
            }
            None => {
                let _ = item.set_text("已是最新版本");
                let _ = item.set_enabled(false);
            }
        }
    }
}

/// Handle menu events by opening the tools window on the requested page.
pub fn handle_menu_event(app: &AppHandle, id: &str) {
    match id {
        // Export/import config run directly (file dialogs), not via tools window.
        MENU_RUN_EXPORT_CONFIG => {
            // Dialog APIs must run on the main thread. The zip packing happens
            // synchronously here (the config is small once node_modules is
            // excluded); a large data set would need a different approach.
            let result = crate::launcher::export_config(app.clone());
            show_config_result(app, result);
            return;
        }
        MENU_RUN_IMPORT_CONFIG => {
            let result = crate::launcher::import_config(app.clone());
            show_config_result(app, result);
            return;
        }
        MENU_VIEW_DEVTOOLS => {
            toggle_devtools(app);
            return;
        }
        MENU_ABOUT_UPDATE_AVAILABLE => {
            // User clicked the "new version" indicator: open the about page and
            // ask it to start the update automatically.
            open_tools_window(app, "about");
            trigger_update_in_about(app);
            return;
        }
        MENU_QUIT => {
            // Kill DSH and quit the whole app.
            let _ = crate::launcher::quit_app(app.clone());
            return;
        }
        _ => {}
    }

    let page = match id {
        MENU_RUN_INSTALL_PLUGIN => "install-plugin",
        MENU_VIEW_LIST_PLUGINS => "list-plugins",
        MENU_SETTINGS => "settings",
        MENU_ABOUT_DSH_VERSION => "dsh-version",
        MENU_ABOUT_CHANGELOG => "changelog",
        MENU_ABOUT_DESKTOP => "about",
        _ => return,
    };

    open_tools_window(app, page);
}

/// Open the tools window on the "about" page and ask it to start the update
/// automatically. Used when the user opts into updating (from the startup
/// prompt or the "new version" menu indicator).
pub fn open_about_with_update(app: &AppHandle) {
    open_tools_window(app, "about");
    trigger_update_in_about(app);
}

/// Run an update check in the background once the main window is ready.
///
/// This is deliberately async and non-blocking: it runs off the UI thread, so
/// it never stalls the embedded DSH web view. When a new version is found it
/// (a) lights up the "发现新版本" menu indicator and (b) shows a yes/no prompt
/// (on the main thread, since native dialogs need it); if the user agrees it
/// opens the about page and triggers the update there.
pub fn spawn_startup_update_check(app: AppHandle) {
    use tauri_plugin_updater::UpdaterExt;
    std::thread::spawn(move || {
        // Let the main window fully settle before surfacing any dialog.
        std::thread::sleep(std::time::Duration::from_secs(2));

        let checked = tauri::async_runtime::block_on(async {
            let updater = app.updater().ok()?;
            updater.check().await.ok().flatten()
        });

        match checked {
            Some(update) => {
                let version = update.version.clone();
                // Light up the menu indicator immediately (thread-safe).
                set_update_available(Some(version.clone()));

                // Ask the user on the main thread (native dialogs need it).
                let app2 = app.clone();
                let _ = app.run_on_main_thread(move || {
                    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
                    let agree = app2
                        .dialog()
                        .message(format!(
                            "发现新版本 v{version},是否立即前往「关于 DeepSeek Harness Desktop」界面并更新?"
                        ))
                        .title("发现新版本")
                        .kind(MessageDialogKind::Info)
                        .buttons(MessageDialogButtons::YesNo)
                        .blocking_show();

                    if agree {
                        open_about_with_update(&app2);
                    }
                });
            }
            None => {
                // No update (or the check failed) — ensure the indicator is
                // reset/disabled so it never shows a stale "new version".
                set_update_available(None);
            }
        }
    });
}

/// Tell the about page to start the update automatically (used when the user
/// opts into updating from the "new version" prompt/indicator).
fn trigger_update_in_about(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        // Give the tools window time to render the about page and register its
        // listener before we tell it to trigger the update.
        std::thread::sleep(std::time::Duration::from_millis(600));
        let _ = app.emit("about-trigger-update", ());
    });
}

/// Show the result of an export/import operation in a message dialog.
fn show_config_result(app: &AppHandle, result: Result<String, String>) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
    match result {
        Ok(msg) => {
            let _ = app.dialog().message(msg).title("配置").blocking_show();
        }
        Err(e) => {
            let _ = app
                .dialog()
                .message(e)
                .title("配置")
                .kind(MessageDialogKind::Error)
                .blocking_show();
        }
    }
}

/// Toggle the DevTools inspector on the main window (if present).
///
/// Called from the "查看 > 调试工具" menu item.
pub fn toggle_devtools(app: &AppHandle) {
    if let Some(main_win) = app.get_webview_window("main") {
        if main_win.is_devtools_open() {
            main_win.close_devtools();
        } else {
            main_win.open_devtools();
        }
    }
}

/// Open (or focus) the tools window on the installed-plugin list page.
///
/// Called by the launcher when DSH fails to start due to a plugin conflict, so
/// the user can manually uninstall the offending plugin.
pub fn open_plugin_list(app: &AppHandle) {
    open_tools_window(app, "list-plugins");
}

/// Open (or focus) the tools window and navigate it to the given page.
fn open_tools_window(app: &AppHandle, page: &str) {
    // Reuse an existing tools window if present.
    if let Some(win) = app.get_webview_window("tools") {
        let _ = win.show();
        let _ = win.set_focus();
        // Non-main windows should not be maximizable.
        let _ = win.set_maximizable(false);
        // Tell the frontend which page to show.
        let _ = app.emit("tools-page", page.to_string());
        return;
    }

    let url = WebviewUrl::App(format!("tools.html?page={page}").into());
    let builder = WebviewWindowBuilder::new(app, "tools", url)
        .title("DeepSeek Harness Desktop 工具")
        .inner_size(560.0, 620.0)
        .resizable(true)
        .maximizable(false)
        .center();

    if let Ok(win) = builder.build() {
        // Belt-and-braces: the initial page is set via the URL query above, so
        // the window shows the right page even before the async emit lands.
        let app2 = app.clone();
        let page = page.to_string();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(400));
            let _ = app2.emit("tools-page", page);
        });
        let _ = win;
    }
}

/// Show (and focus) the main window if it exists.
pub fn show_main_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// Build the system tray icon with a right-click menu.
///
/// Left-clicking the icon shows the main window. The right-click menu reuses
/// the same menu item ids as the native menu, so clicks are routed through
/// `handle_menu_event`.
pub fn build_tray(app: &AppHandle) -> tauri::Result<tauri::tray::TrayIcon> {
    let mut tray = TrayIconBuilder::new()
        .tooltip("DeepSeek Harness Desktop")
        .show_menu_on_left_click(false);

    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }

    // Right-click menu.
    let menu = MenuBuilder::new(app)
        .item(&MenuItemBuilder::with_id(MENU_SETTINGS, "设置").build(app)?)
        .item(&MenuItemBuilder::with_id(MENU_ABOUT_DSH_VERSION, "查看版本").build(app)?)
        .item(&MenuItemBuilder::with_id(MENU_RUN_INSTALL_PLUGIN, "安装插件").build(app)?)
        .item(&MenuItemBuilder::with_id(MENU_VIEW_LIST_PLUGINS, "插件列表").build(app)?)
        .separator()
        .item(&MenuItemBuilder::with_id(MENU_RUN_EXPORT_CONFIG, "导出数据").build(app)?)
        .item(&MenuItemBuilder::with_id(MENU_RUN_IMPORT_CONFIG, "导入数据").build(app)?)
        .separator()
        .item(&MenuItemBuilder::with_id(MENU_QUIT, "退出").build(app)?)
        .build()?;
    tray = tray.menu(&menu);

    tray = tray.on_tray_icon_event(|tray, event| {
        // Left-click shows the main window (right-click still opens the menu).
        if let TrayIconEvent::Click {
            button: tauri::tray::MouseButton::Left,
            ..
        } = event
        {
            show_main_window(tray.app_handle());
        }
    });

    let tray = tray.build(app)?;
    // Keep the tray alive for the app's lifetime.
    *TRAY.lock().unwrap() = Some(tray.clone());
    Ok(tray)
}
