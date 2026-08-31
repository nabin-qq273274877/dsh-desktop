//! DSH launcher: spawns the bundled Node binary to run
//! `npx -y --verbose @deepseek-ai/dsh web --no-open`, streams its stdout/stderr
//! to the loading window, and waits for the web server to become ready.
//!
//! Uses `std::process` for the child (synchronous spawn/kill, cross-platform),
//! with dedicated threads reading the piped stdout/stderr line by line.

use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager};

/// The host DSH binds to. We bind to loopback only for local use.
const DSH_HOST: &str = "127.0.0.1";

/// Global handle to the running child process so we can kill it on exit.
static CHILD: Mutex<Option<Child>> = Mutex::new(None);

/// Whether DSH has been detected as ready (guards against duplicate launches).
static READY: AtomicBool = AtomicBool::new(false);

/// The dynamically chosen port for the current DSH run.
static CURRENT_PORT: Mutex<Option<u16>> = Mutex::new(None);

/// Find a free TCP port by binding to `127.0.0.1:0` and letting the OS choose.
fn find_free_port() -> Result<u16, String> {
    let listener = TcpListener::bind((DSH_HOST, 0u16))
        .map_err(|e| format!("failed to find a free port: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("failed to read assigned port: {e}"))?
        .port();
    // Dropping the listener releases the port back to the OS for DSH to grab.
    drop(listener);
    Ok(port)
}

/// The DSH web URL for the currently chosen port.
fn dsh_url(port: u16) -> String {
    format!("http://{DSH_HOST}:{port}")
}

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
    let port = CURRENT_PORT.lock().unwrap().unwrap_or(0);
    let _ = app.emit("dsh-ready", dsh_url(port));
}

/// Start the DSH child process using the bundled Node binary and stream logs.
pub fn launch_dsh(app: &AppHandle) -> Result<(), String> {
    if READY.load(Ordering::SeqCst) {
        return Ok(());
    }

    let node_path = bundled_node_path(app)?;
    let npx_cli = bundled_npx_path(&node_path)?;

    // Choose a free port so we never collide with a fixed port already in use.
    let port = find_free_port()?;
    *CURRENT_PORT.lock().unwrap() = Some(port);

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
            "$ npx -y --verbose @deepseek-ai/dsh web --port {port} --no-open\n   (node: {})\n   (npx: {})",
            node_path.display(),
            npx_cli.display()
        ),
    );

    let mut cmd = Command::new(&node_path);
    cmd.arg(&npx_cli)
        .args(["-y", "--verbose", "@deepseek-ai/dsh", "web"])
        .arg("--port")
        .arg(port.to_string())
        .arg("--no-open")
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

/// Poll the DSH web URL until it returns 200, then signal readiness.
fn poll_ready(app: AppHandle) {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .ok();

    let port = CURRENT_PORT.lock().unwrap().unwrap_or(0);
    let url = dsh_url(port);

    let mut attempts = 0u32;
    loop {
        attempts += 1;
        std::thread::sleep(std::time::Duration::from_millis(800));

        if let Some(client) = &client {
            match client.get(&url).send() {
                Ok(resp) if resp.status().is_success() => {
                    READY.store(true, Ordering::SeqCst);
                    emit_log(&app, &format!("[ready] DSH web server is up at {url}"));
                    emit_ready(&app);
                    return;
                }
                _ => {
                    if attempts % 5 == 0 {
                        emit_log(&app, &format!("[waiting] {url} not ready yet..."));
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
    *CURRENT_PORT.lock().unwrap() = None;
}

/// Tauri command: frontend can request a (re)start of the DSH launch flow.
#[tauri::command]
pub fn start_dsh(app: AppHandle) -> Result<(), String> {
    READY.store(false, Ordering::SeqCst);
    kill_dsh();
    launch_dsh(&app)
}

/// Tauri command: return the current DSH web URL (for the main window to embed).
#[tauri::command]
pub fn get_dsh_url() -> String {
    let port = CURRENT_PORT.lock().unwrap().unwrap_or(0);
    dsh_url(port)
}
