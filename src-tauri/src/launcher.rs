//! DSH launcher: spawns the bundled Node binary to run
//! `npx -y --verbose @deepseek-ai/dsh web --no-open`, streams its stdout/stderr
//! to the loading window, and waits for the web server to become ready.
//!
//! Uses `std::process` for the child (synchronous spawn/kill, cross-platform),
//! with dedicated threads reading the piped stdout/stderr line by line.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager};

/// The DSH web URL we wait for and then embed in the main window.
const DSH_URL: &str = "http://127.0.0.1:3080";

/// Global handle to the running child process so we can kill it on exit.
static CHILD: Mutex<Option<Child>> = Mutex::new(None);

/// Whether DSH has been detected as ready (guards against duplicate launches).
static READY: AtomicBool = AtomicBool::new(false);

/// Resolve the path to the bundled Node binary.
///
/// At runtime this lives under the app's resource directory (see
/// `tauri.conf.json` -> `bundle.resources.node`). On Windows the binary is
/// `node.exe`, elsewhere `node`.
fn bundled_node_path(app: &AppHandle) -> Result<PathBuf, String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|e| format!("failed to resolve resource dir: {e}"))?;

    #[cfg(target_os = "windows")]
    let node_bin = resource_dir.join("node").join("node.exe");
    #[cfg(not(target_os = "windows"))]
    let node_bin = resource_dir.join("node").join("bin").join("node");

    if !node_bin.exists() {
        return Err(format!("bundled node not found at {}", node_bin.display()));
    }
    Ok(node_bin)
}

/// Resolve the npm npx CLI shipped with the Node distribution.
///
/// Layout differs between platforms:
///   Windows: node/node_modules/npm/bin/npx-cli.js
///   macOS:   node/lib/node_modules/npm/bin/npx-cli.js
fn bundled_npx_path(node_path: &PathBuf) -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    let npm_rel = ["node_modules", "npm", "bin", "npx-cli.js"];
    #[cfg(not(target_os = "windows"))]
    let npm_rel = ["lib", "node_modules", "npm", "bin", "npx-cli.js"];

    let mut path = node_path.parent().unwrap().to_path_buf();
    for seg in npm_rel {
        path.push(seg);
    }
    if !path.exists() {
        return Err(format!("bundled npm npx-cli not found at {}", path.display()));
    }
    Ok(path)
}

/// Emit a single log line to the loading window.
fn emit_log(app: &AppHandle, line: &str) {
    let _ = app.emit("dsh-log", line.trim_end().to_string());
}

/// Notify the frontend that DSH is ready and the main window should open.
fn emit_ready(app: &AppHandle) {
    let _ = app.emit("dsh-ready", DSH_URL.to_string());
}

/// Start the DSH child process using the bundled Node binary and stream logs.
pub fn launch_dsh(app: &AppHandle) -> Result<(), String> {
    if READY.load(Ordering::SeqCst) {
        return Ok(());
    }

    let node_path = bundled_node_path(app)?;
    let npx_cli = bundled_npx_path(&node_path)?;

    // Use an isolated cache/prefix so we never touch the user's system npm.
    // Named "dsh-desktop" for easy discovery when debugging.
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("failed to resolve app data dir: {e}"))?;
    let npm_cache = data_dir.join("dsh-desktop-cache");
    let npm_prefix = data_dir.join("dsh-desktop-prefix");

    emit_log(
        app,
        &format!(
            "$ npx -y --verbose @deepseek-ai/dsh web --no-open\n   (node: {})\n   (npx: {})",
            node_path.display(),
            npx_cli.display()
        ),
    );

    let mut cmd = Command::new(&node_path);
    cmd.arg(&npx_cli)
        .args(["-y", "--verbose", "@deepseek-ai/dsh", "web", "--no-open"])
        .env("npm_config_cache", &npm_cache)
        .env("npm_config_prefix", &npm_prefix)
        .env("NODE_ENV", "production")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: run without a console window.
        cmd.creation_flags(0x0800_0000);
    }

    let mut child = cmd.spawn().map_err(|e| {
        emit_log(app, &format!("[error] failed to spawn DSH: {e}"));
        e.to_string()
    })?;

    // Take the piped streams before moving child into the Mutex.
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    *CHILD.lock().unwrap() = Some(child);

    let app_stdout = app.clone();
    if let Some(out) = stdout {
        std::thread::spawn(move || stream_lines(app_stdout, out));
    }
    let app_stderr = app.clone();
    if let Some(err) = stderr {
        std::thread::spawn(move || stream_lines(app_stderr, err));
    }

    // Poll for readiness in the background.
    let app_poll = app.clone();
    std::thread::spawn(move || poll_ready(app_poll));

    Ok(())
}

/// Stream lines from a child process pipe to the loading window.
fn stream_lines(app: AppHandle, stream: impl std::io::Read + Send + 'static) {
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        match line {
            Ok(l) => emit_log(&app, &l),
            Err(_) => break,
        }
    }
}

/// Poll `http://127.0.0.1:3080` until it returns 200, then signal readiness.
fn poll_ready(app: AppHandle) {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .ok();

    let mut attempts = 0u32;
    loop {
        attempts += 1;
        std::thread::sleep(std::time::Duration::from_millis(800));

        if let Some(client) = &client {
            match client.get(DSH_URL).send() {
                Ok(resp) if resp.status().is_success() => {
                    READY.store(true, Ordering::SeqCst);
                    emit_log(&app, "[ready] DSH web server is up");
                    emit_ready(&app);
                    return;
                }
                _ => {
                    if attempts % 5 == 0 {
                        emit_log(&app, "[waiting] DSH web server not ready yet...");
                    }
                }
            }
        }
    }
}

/// Kill the DSH child process if it is still running.
pub fn kill_dsh() {
    if let Some(mut child) = CHILD.lock().unwrap().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Tauri command: frontend can request a (re)start of the DSH launch flow.
#[tauri::command]
pub fn start_dsh(app: AppHandle) -> Result<(), String> {
    READY.store(false, Ordering::SeqCst);
    kill_dsh();
    launch_dsh(&app)
}
