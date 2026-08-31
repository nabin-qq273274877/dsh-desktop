//! Native application menu: "运行" (install plugin), "查看" (list plugins),
//! and "关于" (desktop version / DSH version / check update).
//!
//! Menu clicks open a dedicated HTML "tools" window and tell it which page to
//! show. The tools window performs the actual work via Tauri commands.

use tauri::menu::{Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

const MENU_RUN_INSTALL_PLUGIN: &str = "run_install_plugin";
const MENU_VIEW_LIST_PLUGINS: &str = "view_list_plugins";
const MENU_ABOUT_CHECK_UPDATE: &str = "about_check_update";
const MENU_ABOUT_DSH_VERSION: &str = "about_dsh_version";
const MENU_ABOUT_DESKTOP: &str = "about_desktop";

/// Build the native app menu.
pub fn build_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let run_submenu = SubmenuBuilder::new(app, "运行")
        .item(
            &MenuItemBuilder::with_id(MENU_RUN_INSTALL_PLUGIN, "安装插件…")
                .build(app)?,
        )
        .build()?;

    let view_submenu = SubmenuBuilder::new(app, "查看")
        .item(
            &MenuItemBuilder::with_id(MENU_VIEW_LIST_PLUGINS, "已安装插件")
                .build(app)?,
        )
        .build()?;

    let about_submenu = SubmenuBuilder::new(app, "关于")
        .item(
            &MenuItemBuilder::with_id(MENU_ABOUT_CHECK_UPDATE, "检查更新…")
                .build(app)?,
        )
        .item(
            &MenuItemBuilder::with_id(MENU_ABOUT_DSH_VERSION, "DSH 版本")
                .build(app)?,
        )
        .item(
            &MenuItemBuilder::with_id(MENU_ABOUT_DESKTOP, "关于 DSH Desktop")
                .build(app)?,
        )
        .build()?;

    let menu = MenuBuilder::new(app)
        .items(&[&run_submenu, &view_submenu, &about_submenu])
        .build()?;

    Ok(menu)
}

/// Handle menu events by opening the tools window on the requested page.
pub fn handle_menu_event(app: &AppHandle, id: &str) {
    let page = match id {
        MENU_RUN_INSTALL_PLUGIN => "install-plugin",
        MENU_VIEW_LIST_PLUGINS => "list-plugins",
        MENU_ABOUT_CHECK_UPDATE => "check-update",
        MENU_ABOUT_DSH_VERSION => "dsh-version",
        MENU_ABOUT_DESKTOP => "about",
        _ => return,
    };

    open_tools_window(app, page);
}

/// Open (or focus) the tools window and navigate it to the given page.
fn open_tools_window(app: &AppHandle, page: &str) {
    // Reuse an existing tools window if present.
    if let Some(win) = app.get_webview_window("tools") {
        let _ = win.show();
        let _ = win.set_focus();
        // Tell the frontend which page to show.
        let _ = app.emit("tools-page", page.to_string());
        return;
    }

    let url = WebviewUrl::App("tools.html".into());
    let builder = WebviewWindowBuilder::new(app, "tools", url)
        .title("DSH Desktop 工具")
        .inner_size(560.0, 560.0)
        .resizable(true)
        .center();

    if let Ok(win) = builder.build() {
        // Emit after a short delay so the frontend has registered its listener.
        let app2 = app.clone();
        let page = page.to_string();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(400));
            let _ = app2.emit("tools-page", page);
        });
        let _ = win;
    }
}
