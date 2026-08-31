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

/// Rolling buffer of log lines so a late-arriving frontend can replay history.
static LOG_HISTORY: Mutex<Vec<String>> = Mutex::new(Vec::new());

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
/// At runtime this lives under the app's resource directory. In a packaged
/// build the resource maps to `<resource_dir>/node/`; in dev mode the raw
/// source layout `<src-tauri>/resources/node/` is used. We try both.
fn bundled_node_path(app: &AppHandle) -> Result<PathBuf, String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|e| format!("failed to resolve resource dir: {e}"))?;

    #[cfg(target_os = "windows")]
    let rel: &[&str] = &["node.exe"];
    #[cfg(not(target_os = "windows"))]
    let rel: &[&str] = &["bin", "node"];

    let mut candidates: Vec<PathBuf> = vec![];
    // Packaged: resource_dir/node/...
    candidates.push({
        let mut p = resource_dir.clone();
        p.push("node");
        for seg in rel {
            p.push(seg);
        }
        p
    });
    // Dev: resource_dir (src-tauri) / resources/node/...
    candidates.push({
        let mut p = resource_dir.clone();
        p.push("resources");
        p.push("node");
        for seg in rel {
            p.push(seg);
        }
        p
    });

    for c in &candidates {
        if c.exists() {
            return Ok(c.clone());
        }
    }
    Err(format!(
        "bundled node not found. Tried:\n  {}",
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n  ")
    ))
}

/// Resolve the bundled pnpm entry script (`bin/pnpm.mjs`).
///
/// pnpm is placed beside the Node distribution at `<node_dir>/pnpm/bin/pnpm.mjs`
/// and run via the bundled Node (`node pnpm.mjs dlx ...`). This avoids relying on
/// Node's built-in npm/npx and gives faster, hard-linked installs.
fn bundled_pnpm_path(node_path: &PathBuf) -> Result<PathBuf, String> {
    let mut path = node_path.parent().unwrap().to_path_buf();
    path.push("pnpm");
    path.push("bin");
    path.push("pnpm.mjs");
    if !path.exists() {
        return Err(format!("bundled pnpm not found at {}", path.display()));
    }
    Ok(path)
}

/// Emit a single log line to the loading window, and buffer it for replay.
fn emit_log(app: &AppHandle, line: &str) {
    let line = line.trim_end().to_string();
    // Buffer (capped) so a late-arriving frontend can replay history.
    {
        let mut hist = LOG_HISTORY.lock().unwrap();
        hist.push(line.clone());
        if hist.len() > 5000 {
            let excess = hist.len() - 5000;
            hist.drain(0..excess);
        }
    }
    let _ = app.emit("dsh-log", line);
}

/// Notify the frontend that DSH is ready and open the main window.
///
/// We navigate the "main" window directly to the DSH URL (no iframe), then show
/// it and emit `dsh-ready` so the loading window hides itself.
fn emit_ready(app: &AppHandle) {
    let port = CURRENT_PORT.lock().unwrap().unwrap_or(0);
    let url = dsh_url(port);

    // Navigate and show the main window directly.
    if let Some(main_win) = app.get_webview_window("main") {
        if let Ok(parsed) = url::Url::parse(&url) {
            let _ = main_win.navigate(parsed);
        }
        let _ = main_win.show();
        let _ = main_win.set_focus();
    }

    let _ = app.emit("dsh-ready", url);
}

/// Start the DSH child process using the bundled Node binary and stream logs.
pub fn launch_dsh(app: &AppHandle) -> Result<(), String> {
    if READY.load(Ordering::SeqCst) {
        return Ok(());
    }

    let node_path = bundled_node_path(app)?;
    let pnpm_path = bundled_pnpm_path(&node_path)?;

    // Choose a free port so we never collide with a fixed port already in use.
    let port = find_free_port()?;
    *CURRENT_PORT.lock().unwrap() = Some(port);

    // Use an isolated store/cache so we never touch the user's global pnpm/npm.
    // Everything lives under a single "dsh-desktop" directory for easy
    // discovery, with "store" and "cache" subdirectories.
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("failed to resolve app data dir: {e}"))?;
    let dsh_dir = data_dir.join("dsh-desktop");
    let pnpm_store = dsh_dir.join("store");
    let dsh_home = dsh_dir.join("dsh-home");

    emit_log(
        app,
        &format!(
            "$ pnpm dlx @deepseek-ai/dsh web --port {port} --no-open\n   (node: {})\n   (pnpm: {})",
            node_path.display(),
            pnpm_path.display()
        ),
    );

    let mut cmd = Command::new(&node_path);
    cmd.arg(&pnpm_path)
        // Non-interactive, line-based reporter: required because we pipe
        // stdout/stderr (the default TTY reporter misbehaves on a pipe).
        .arg("--reporter=append-only")
        .arg("dlx")
        .arg("@deepseek-ai/dsh")
        .arg("web")
        .arg("--port")
        .arg(port.to_string())
        .arg("--no-open")
        // Isolated pnpm store + cache under the app data dir.
        .env("PNPM_HOME", &dsh_dir)
        .env("npm_config_store_dir", &pnpm_store)
        .env("npm_config_cache", &dsh_dir.join("cache"))
        // Isolated DSH home: keeps config/plugins/sessions separate from any
        // other DSH install on the machine (no lock collisions, no pollution).
        .env("DSH_HOME", &dsh_home)
        .env("HOME", &dsh_home)
        // Use the npmmirror registry for faster installs in CN networks.
        .env("npm_config_registry", "https://registry.npmmirror.com")
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

/// Tauri command: return all buffered log lines (so the loading window can
/// replay history that was emitted before its JS listener was ready).
#[tauri::command]
pub fn get_log_history() -> Vec<String> {
    LOG_HISTORY.lock().unwrap().clone()
}
