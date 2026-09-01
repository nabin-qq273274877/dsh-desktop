//! Persisted user settings for the desktop launcher.
//!
//! Two options are exposed (via the 设置 menu / tools settings page):
//!   * `launcher`      — `"pnpm"` (default) or `"npx"`: which package runner is
//!                       used to fetch and run `@deepseek-ai/dsh`.
//!   * `version_channel` — `"latest"` (default), `"next"` or `"alpha"`: which
//!                       dist-tag of `@deepseek-ai/dsh` is installed.
//!
//! Settings are stored as a small JSON file under the app data directory so they
//! survive restarts. A static default (allowing a no-disk fast path) is kept so
//! the launcher works even if the file cannot be read.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

/// Valid package-runner values.
pub const LAUNCHER_PNPM: &str = "pnpm";
pub const LAUNCHER_NPX: &str = "npx";

/// Valid version-channel (dist-tag) values.
pub const CHANNEL_LATEST: &str = "latest";
pub const CHANNEL_NEXT: &str = "next";
pub const CHANNEL_ALPHA: &str = "alpha";

/// Valid close-behavior values.
pub const CLOSE_QUIT: &str = "quit";
pub const CLOSE_TRAY: &str = "tray";

/// Default settings used until the on-disk file is read.
fn default_settings() -> Settings {
    Settings {
        launcher: LAUNCHER_PNPM.to_string(),
        version_channel: CHANNEL_LATEST.to_string(),
        close_behavior: CLOSE_QUIT.to_string(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Settings {
    /// `"pnpm"` or `"npx"`.
    pub launcher: String,
    /// `"latest"` | `"next"` | `"alpha"`.
    pub version_channel: String,
    /// `"quit"` (close main window exits) or `"tray"` (close hides to tray).
    #[serde(default = "default_close_behavior")]
    pub close_behavior: String,
}

fn default_close_behavior() -> String {
    CLOSE_QUIT.to_string()
}

impl Settings {
    /// Whether the runner resolves to `npx` (defaults to pnpm for unknown values).
    pub fn uses_npx(&self) -> bool {
        self.launcher == LAUNCHER_NPX
    }

    /// Whether closing the main window should hide to tray instead of exiting.
    pub fn close_to_tray(&self) -> bool {
        self.close_behavior == CLOSE_TRAY
    }
}

/// The settings file path under the app data dir.
fn settings_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("failed to resolve app data dir: {e}"))?;
    Ok(data_dir.join("dsh-desktop").join("settings.json"))
}

/// Cache of the in-memory settings so reads are cheap and never hit disk more
/// than needed. Kept in sync with the file on every save.
static CACHED: Mutex<Option<Settings>> = Mutex::new(None);

/// Read the current settings, loading from disk (and caching) on first use.
pub fn get(app: &AppHandle) -> Settings {
    {
        let guard = CACHED.lock().unwrap();
        if let Some(s) = guard.as_ref() {
            return s.clone();
        }
    }

    let loaded = match settings_path(app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
    {
        Some(s) => s,
        None => default_settings(),
    };
    *CACHED.lock().unwrap() = Some(loaded.clone());
    loaded
}

/// Persist the given settings to disk and update the in-memory cache.
pub fn save(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let path = settings_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("failed to create settings dir: {e}"))?;
    }
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("failed to write settings: {e}"))?;
    *CACHED.lock().unwrap() = Some(settings.clone());
    Ok(())
}

/// Validate a launcher value; falls back to the default on invalid input.
pub fn normalize_launcher(v: &str) -> String {
    match v {
        LAUNCHER_NPX | LAUNCHER_PNPM => v.to_string(),
        _ => LAUNCHER_PNPM.to_string(),
    }
}

/// Validate a version-channel value; falls back to `latest` on invalid input.
pub fn normalize_channel(v: &str) -> String {
    match v {
        CHANNEL_NEXT | CHANNEL_ALPHA => v.to_string(),
        _ => CHANNEL_LATEST.to_string(),
    }
}

/// Validate a close-behavior value; falls back to `quit` on invalid input.
pub fn normalize_close_behavior(v: &str) -> String {
    match v {
        CLOSE_TRAY => v.to_string(),
        _ => CLOSE_QUIT.to_string(),
    }
}

/// Tauri command: return the current settings as a JSON object.
#[tauri::command]
pub fn get_settings(app: AppHandle) -> Settings {
    get(&app)
}

/// Tauri command: update settings. Accepts optional fields so the frontend can
/// change one value without resending the other. Invalid values fall back to
/// defaults.
#[tauri::command]
pub fn update_settings(
    app: AppHandle,
    launcher: Option<String>,
    version_channel: Option<String>,
    close_behavior: Option<String>,
) -> Result<Settings, String> {
    let mut s = get(&app);
    if let Some(l) = launcher {
        s.launcher = normalize_launcher(&l);
    }
    if let Some(c) = version_channel {
        s.version_channel = normalize_channel(&c);
    }
    if let Some(cb) = close_behavior {
        s.close_behavior = normalize_close_behavior(&cb);
    }
    save(&app, &s)?;
    Ok(s)
}
