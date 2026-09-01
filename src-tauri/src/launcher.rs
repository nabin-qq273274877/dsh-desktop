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

/// Set when DSH logs indicate a plugin loading conflict (e.g. a plugin imports
/// something the running DSH version doesn't export), so the launcher can offer
/// the plugin list for manual uninstall instead of leaving the user stuck.
static PLUGIN_CONFLICT: AtomicBool = AtomicBool::new(false);

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
pub(crate) fn bundled_node_path(app: &AppHandle) -> Result<PathBuf, String> {
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

/// The Node distribution root directory (contains `bin/node` on Unix, or
/// `node.exe` on Windows, plus the bundled `pnpm/` and `node_modules/npm/`).
///
/// On Unix the node binary lives at `<dist>/bin/node`; on Windows at
/// `<dist>/node.exe`. Everything else (pnpm, npm) sits in the dist root, so we
/// must walk back to that root before looking for siblings — using
/// `node_path.parent()` alone would wrongly add a `bin/` level on Unix.
fn bundled_node_dir(node_path: &PathBuf) -> PathBuf {
    let name = node_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if name == "node.exe" {
        // Windows: <dist>/node.exe
        node_path.parent().unwrap().to_path_buf()
    } else {
        // Unix: <dist>/bin/node -> <dist>
        node_path
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| node_path.parent().unwrap().to_path_buf())
    }
}

/// Prepend the bundled Node binary's directory to the child's `PATH`.
///
/// pnpm's postinstall/build scripts (`node-pty`, `koffi`, `protobufjs`, ...)
/// invoke `node` via the system `PATH`. On macOS the system usually has no
/// `node`, so those scripts fail with `sh: node: command not found`. Adding the
/// bundled node's directory (where `node`/`node.exe` lives) to the front of
/// `PATH` makes those scripts resolve our bundled node instead.
fn prepend_node_to_path(cmd: &mut Command, node_path: &PathBuf) {
    let Some(node_dir) = node_path.parent() else {
        return;
    };
    let node_dir = node_dir.to_path_buf();
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = std::env::split_paths(&existing).collect::<Vec<_>>();
    paths.insert(0, node_dir.clone());
    if let Ok(joined) = std::env::join_paths(&paths) {
        cmd.env("PATH", joined);
    } else {
        cmd.env("PATH", &existing);
    }
}

/// Resolve the bundled pnpm entry script (`bin/pnpm.mjs`).
///
/// pnpm is placed beside the Node distribution at `<node_dir>/pnpm/bin/pnpm.mjs`
/// and run via the bundled Node (`node pnpm.mjs dlx ...`). This avoids relying on
/// Node's built-in npm/npx and gives faster, hard-linked installs.
fn bundled_pnpm_path(node_path: &PathBuf) -> Result<PathBuf, String> {
    let mut path = bundled_node_dir(node_path);
    path.push("pnpm");
    path.push("bin");
    path.push("pnpm.mjs");
    if !path.exists() {
        return Err(format!("bundled pnpm not found at {}", path.display()));
    }
    Ok(path)
}

/// Resolve the bundled npm npx CLI (`<node_dir>/node_modules/npm/bin/npx-cli.js`).
///
/// The bundled Node ships with npm, so when the user picks `npx` as the runner
/// we invoke that bundled npx directly (`node npx-cli.js -y ...`) rather than
/// relying on a system-wide npx install.
fn bundled_npx_path(node_path: &PathBuf) -> Result<PathBuf, String> {
    let mut path = bundled_node_dir(node_path);
    path.push("node_modules");
    path.push("npm");
    path.push("bin");
    path.push("npx-cli.js");
    if !path.exists() {
        return Err(format!("bundled npx not found at {}", path.display()));
    }
    Ok(path)
}

/// Resolve the DSH home data directory (`<app-data>/dsh-desktop/dsh-home`).
pub(crate) fn dsh_home_path(app: &AppHandle) -> Result<PathBuf, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("failed to resolve app data dir: {e}"))?;
    Ok(data_dir.join("dsh-desktop").join("dsh-home"))
}

/// Build a `Command` that runs `@deepseek-ai/dsh <args>` through the configured
/// runner (`node pnpm.mjs dlx` or bundled `npx`), with the isolated
/// store/cache/DSH-home env, suitable for one-shot subcommands (`--version`,
/// `plugin --profile web add <pkg>`, ...). The version channel (dist-tag) is
/// taken from the current settings.
pub(crate) fn dsh_subcommand(app: &AppHandle, args: &[&str]) -> Result<Command, String> {
    let settings = crate::settings::get(app);
    let node_path = bundled_node_path(app)?;

    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("failed to resolve app data dir: {e}"))?;
    let dsh_dir = data_dir.join("dsh-desktop");
    let pnpm_store = dsh_dir.join("store");
    let dsh_home = dsh_dir.join("dsh-home");

    // Ensure the data dirs exist (pnpm lstat's `$HOME`/store; a missing dir on
    // macOS surfaces as `ENOENT: lstat .../com.dsh.desktop`).
    let _ = std::fs::create_dir_all(&dsh_home);
    let _ = std::fs::create_dir_all(&pnpm_store);
    let _ = std::fs::create_dir_all(dsh_dir.join("cache"));

    let dsh_pkg = format!("@deepseek-ai/dsh@{}", settings.version_channel);

    let mut cmd = Command::new(&node_path);
    if settings.uses_npx() {
        // Use the bundled npm's npx CLI directly.
        let npx_path = bundled_npx_path(&node_path)?;
        cmd.arg(&npx_path).arg("-y").arg(&dsh_pkg);
    } else {
        let pnpm_path = bundled_pnpm_path(&node_path)?;
        cmd.arg(&pnpm_path)
            .arg("--reporter=append-only")
            // Auto-approve build scripts so pnpm never drops into the
            // interactive "Choose which packages to build" prompt (which hangs
            // when piped / non-TTY).
            .arg("--config.dangerouslyAllowAllBuilds=true")
            .arg("dlx")
            .arg(&dsh_pkg);
    }
    for a in args {
        cmd.arg(a);
    }
    // Ensure pnpm postinstall/build scripts can find `node` (esp. on macOS
    // where the system PATH usually has no node).
    prepend_node_to_path(&mut cmd, &node_path);
    cmd.env("PNPM_HOME", &dsh_dir)
        .env("npm_config_store_dir", &pnpm_store)
        .env("npm_config_cache", &dsh_dir.join("cache"))
        .env("DSH_HOME", &dsh_home)
        .env("HOME", &dsh_home)
        .env("npm_config_registry", "https://registry.npmmirror.com")
        .env("NODE_ENV", "production")
        // Network resilience: npmmirror occasionally drops tarball connections
        // (UND_ERR_DESTROYED) for optional platform packages (e.g.
        // lightningcss-darwin-x64) that pnpm still verifies on install. Bump the
        // retry count and shorten the backoff floor so a transient failure does
        // not abort the whole plugin install (which otherwise surfaces as
        // "pnpm failed in profile directory" + a frozen UI).
        .env("npm_config_fetch_retries", "5")
        .env("npm_config_fetch_retry_mintimeout", "2000")
        .env("npm_config_fetch_retry_maxtimeout", "30000")
        .env("npm_config_fetch_timeout", "120000")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    Ok(cmd)
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

    // Kick off the async "new version" check now that the main window is up.
    // It runs off the UI thread so it never blocks the embedded DSH view.
    crate::menu::spawn_startup_update_check(app.clone());
}

/// Start the DSH child process using the bundled Node binary and stream logs.
pub fn launch_dsh(app: &AppHandle) -> Result<(), String> {
    if READY.load(Ordering::SeqCst) {
        return Ok(());
    }

    let settings = crate::settings::get(app);
    let node_path = bundled_node_path(app)?;

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

    // Ensure the data directories exist before handing them to pnpm. On macOS
    // (and fresh installs) `Library/Application Support/<id>` may not exist yet,
    // and pnpm lstat's `$HOME`/store — a missing dir surfaces as
    // `ENOENT: lstat .../com.dsh.desktop` and the launch aborts.
    std::fs::create_dir_all(&dsh_home)
        .map_err(|e| format!("failed to create DSH home dir {}: {e}", dsh_home.display()))?;
    std::fs::create_dir_all(&pnpm_store)
        .map_err(|e| format!("failed to create pnpm store dir {}: {e}", pnpm_store.display()))?;
    std::fs::create_dir_all(dsh_dir.join("cache"))
        .map_err(|e| format!("failed to create cache dir: {e}"))?;

    let runner = if settings.uses_npx() { "npx" } else { "pnpm" };
    emit_log(
        app,
        &format!(
            "$ {runner} dlx @deepseek-ai/dsh@{} web --port {port} --no-open\n   (node: {})",
            settings.version_channel,
            node_path.display()
        ),
    );

    let dsh_pkg = format!("@deepseek-ai/dsh@{}", settings.version_channel);

    let mut cmd = Command::new(&node_path);
    if settings.uses_npx() {
        let npx_path = bundled_npx_path(&node_path)?;
        cmd.arg(&npx_path)
            .arg("-y")
            .arg(&dsh_pkg)
            .arg("web")
            .arg("--port")
            .arg(port.to_string())
            .arg("--no-open");
    } else {
        let pnpm_path = bundled_pnpm_path(&node_path)?;
        cmd.arg(&pnpm_path)
            // Non-interactive, line-based reporter: required because we pipe
            // stdout/stderr (the default TTY reporter misbehaves on a pipe).
            .arg("--reporter=append-only")
            // Never prompt interactively to approve postinstall/build scripts
            // (node-pty, koffi, ...). In a piped/non-TTY environment that prompt
            // cannot be answered and pnpm hangs at "Choose which packages to
            // build". Auto-approve all build scripts instead.
            .arg("--config.dangerouslyAllowAllBuilds=true")
            .arg("dlx")
            .arg(&dsh_pkg)
            .arg("web")
            .arg("--port")
            .arg(port.to_string())
            .arg("--no-open");
    }
    // Ensure pnpm postinstall/build scripts can find `node` (esp. on macOS
    // where the system PATH usually has no node).
    prepend_node_to_path(&mut cmd, &node_path);
    cmd.env("PNPM_HOME", &dsh_dir)
        .env("npm_config_store_dir", &pnpm_store)
        .env("npm_config_cache", &dsh_dir.join("cache"))
        // Isolated DSH home: keeps config/plugins/sessions separate from any
        // other DSH install on the machine (no lock collisions, no pollution).
        .env("DSH_HOME", &dsh_home)
        .env("HOME", &dsh_home)
        // Use the npmmirror registry for faster installs in CN networks.
        .env("npm_config_registry", "https://registry.npmmirror.com")
        .env("NODE_ENV", "production")
        // Same network-resilience tuning as dsh_subcommand (see comments there).
        .env("npm_config_fetch_retries", "5")
        .env("npm_config_fetch_retry_mintimeout", "2000")
        .env("npm_config_fetch_retry_maxtimeout", "30000")
        .env("npm_config_fetch_timeout", "120000")
        // Ensure the child can never read a TTY from us: stdin is closed so any
        // interactive pnpm/npm prompt fails fast instead of hanging the loading
        // window waiting for input.
        .stdin(Stdio::null())
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

    // On Windows, tie the child to a kill-on-close Job Object so that if this
    // launcher exits by ANY path (normal close, crash, or being killed), the
    // OS terminates the whole child tree — preventing leaked node.exe locks.
    #[cfg(windows)]
    {
        let pid = child.id();
        crate::job_object::assign_process(pid);
    }

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
            Ok(l) => {
                detect_plugin_conflict(&l);
                emit_log(&app, &l);
            }
            Err(_) => break,
        }
    }
}

/// Heuristically detect DSH plugin-loading conflicts in the log stream and flag
/// them so the launcher can surface the plugin list for manual uninstall.
fn detect_plugin_conflict(line: &str) {
    let lower = line.to_lowercase();
    let markers = [
        "plugin tree failed to load",
        "failed to apply loader entry",
        "failed to import loader entry",
        "does not provide an export named",
        "plugin.*load failed",
    ];
    if markers.iter().any(|m| lower.contains(m)) {
        PLUGIN_CONFLICT.store(true, Ordering::SeqCst);
    }
}

/// Poll the DSH web URL until it returns 200, then signal readiness.
///
/// If the DSH child process exits before the web server comes up, the launch is
/// treated as failed: the loop stops (instead of waiting forever) and, if a
/// plugin conflict was detected in the logs, the plugin list window is opened so
/// the user can uninstall the offending plugin.
fn poll_ready(app: AppHandle) {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .ok();

    let port = CURRENT_PORT.lock().unwrap().unwrap_or(0);
    let url = dsh_url(port);

    let mut attempts = 0u32;
    loop {
        // Check for child exit *before* sleeping so a crash is detected as soon
        // as possible (and we stop emitting "[waiting]" spam that would bury the
        // real error message).
        let exited = CHILD
            .lock()
            .unwrap()
            .as_mut()
            .map(|c| matches!(c.try_wait(), Ok(Some(_))))
            .unwrap_or(false);
        if exited && !READY.load(Ordering::SeqCst) {
            emit_log(&app, "[error] DSH 进程已退出,启动失败。");
            // Let the loading window enable the "重试启动" button (the failure
            // is async; start_dsh returned Ok, so the frontend can't detect it).
            let _ = app.emit("dsh-launch-failed", ());
            if PLUGIN_CONFLICT.load(Ordering::SeqCst) {
                emit_log(
                    &app,
                    "[hint] 检测到插件冲突。已为你打开「已安装插件」页面,请卸载有问题的插件后点击「重试启动」。",
                );
                // Reuse the menu helper to surface the plugin list window.
                crate::menu::open_plugin_list(&app);
            } else {
                emit_log(&app, "[hint] 可点击下方「重试启动」再次尝试。");
            }
            return;
        }

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
                    // "waiting" is only a faint heartbeat so the user knows the
                    // app is still working (deps may take a while to download).
                    // Emit it rarely and never spam it, so a real error isn't
                    // pushed off-screen.
                    if attempts == 5 {
                        emit_log(&app, &format!("[waiting] {url} not ready yet,正在下载依赖,请稍候…"));
                    } else if attempts % 50 == 0 {
                        emit_log(&app, &format!("[waiting] {url} 仍在等待,请耐心等待…"));
                    }
                }
            }
        }
    }
}

/// Kill the DSH child process (and its descendants) if still running.
///
/// This is intentionally non-blocking: it must be callable from window-close
/// and exit handlers without freezing the UI. The process tree is terminated
/// in the background via `taskkill /T /F`.
pub fn kill_dsh() {
    if let Some(mut child) = CHILD.lock().unwrap().take() {
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            // Kill the whole process tree asynchronously (DSH spawns cmd.exe /
            // nested node), otherwise descendants leak after the app exits.
            let pid = child.id();
            let _ = Command::new("taskkill")
                .args(["/T", "/F", "/PID", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW: no console flash
                .spawn(); // fire-and-forget; do NOT block the UI thread
            // Fallback kill of the direct child (non-blocking).
            let _ = child.kill();
            let _ = child.try_wait();
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = child.kill();
            let _ = child.try_wait();
        }
    }
    *CURRENT_PORT.lock().unwrap() = None;
}

/// Tauri command: frontend can request a (re)start of the DSH launch flow.
#[tauri::command]
pub fn start_dsh(app: AppHandle) -> Result<(), String> {
    READY.store(false, Ordering::SeqCst);
    PLUGIN_CONFLICT.store(false, Ordering::SeqCst);
    kill_dsh();
    launch_dsh(&app)
}

/// Tauri command: kill DSH and quit the whole app. Used by the loading window's
/// "退出" button so clicking it truly stops the launcher (and the DSH child
/// process) instead of just closing the loading window and letting DSH start in
/// the background.
#[tauri::command]
pub fn quit_app(app: AppHandle) -> Result<(), String> {
    kill_dsh();
    app.exit(0);
    Ok(())
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

/// Run a one-shot DSH subcommand and return its combined stdout+stderr.
fn run_dsh_command(app: &AppHandle, args: &[&str]) -> Result<String, String> {
    let output = dsh_subcommand(app, args)?
        .output()
        .map_err(|e| format!("failed to run dsh: {e}"))?;
    let mut combined = String::new();
    combined.push_str(&String::from_utf8_lossy(&output.stdout));
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        return Err(if combined.trim().is_empty() {
            format!("command exited with status {}", output.status)
        } else {
            combined.trim().to_string()
        });
    }
    Ok(combined)
}

/// Tauri command: return the DSH version string (`dsh --version`).
#[tauri::command]
pub fn get_dsh_version(app: AppHandle) -> Result<String, String> {
    run_dsh_command(&app, &["--version"]).map(|s| s.trim().to_string())
}

/// Run a DSH subcommand off the main thread so a long-running install
/// (pnpm dependency download + retries) never freezes the UI. This mirrors
/// `run_dsh_command` but yields to the async runtime instead of blocking.
async fn run_dsh_command_async(
    app: tauri::AppHandle,
    args: Vec<String>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        run_dsh_command(&app, &refs)
    })
    .await
    .map_err(|e| format!("command task failed: {e}"))?
}

/// Tauri command: list installed plugins (`dsh plugin --profile web list`).
#[tauri::command]
pub async fn list_plugins(app: AppHandle) -> Result<String, String> {
    run_dsh_command_async(
        app,
        vec!["plugin".into(), "--profile".into(), "web".into(), "list".into()],
    )
    .await
}

/// Tauri command: install a plugin by package name.
///
/// On failure, the partially-installed plugin is automatically removed so a
/// broken half-install never lingers in the profile.
#[tauri::command]
pub async fn install_plugin(app: AppHandle, package: String) -> Result<String, String> {
    let pkg = package.trim().to_string();
    if pkg.is_empty() {
        return Err("package name is empty".to_string());
    }
    match run_dsh_command_async(
        app.clone(),
        vec![
            "plugin".into(),
            "--profile".into(),
            "web".into(),
            "add".into(),
            pkg.clone(),
        ],
    )
    .await
    {
        Ok(out) => Ok(out),
        Err(e) => {
            // Best-effort cleanup: uninstall the failed plugin so the profile is
            // not left in a broken/partial state. Ignore cleanup errors — the
            // original error is what matters.
            let _ = run_dsh_command_async(
                app,
                vec![
                    "plugin".into(),
                    "--profile".into(),
                    "web".into(),
                    "remove".into(),
                    pkg.clone(),
                ],
            )
            .await;
            Err(format!("{e}"))
        }
    }
}

/// Tauri command: remove a plugin by package name.
#[tauri::command]
pub async fn remove_plugin(app: AppHandle, package: String) -> Result<String, String> {
    let pkg = package.trim().to_string();
    if pkg.is_empty() {
        return Err("package name is empty".to_string());
    }
    run_dsh_command_async(
        app,
        vec![
            "plugin".into(),
            "--profile".into(),
            "web".into(),
            "remove".into(),
            pkg,
        ],
    )
    .await
}

/// Tauri command: update a plugin to its latest version.
///
/// DSH updates a plugin the same way it installs it — `add <pkg>@latest` — so
/// the pinned package is refreshed to the latest published version.
#[tauri::command]
pub async fn update_plugin(app: AppHandle, package: String) -> Result<String, String> {
    let pkg = package.trim().to_string();
    if pkg.is_empty() {
        return Err("package name is empty".to_string());
    }
    // Scope packages (@scope/name) need `@scope/name@latest`, so append the tag
    // after the full package name.
    let target = format!("{pkg}@latest");
    run_dsh_command_async(
        app,
        vec![
            "plugin".into(),
            "--profile".into(),
            "web".into(),
            "add".into(),
            target,
        ],
    )
    .await
}

/// Export the DSH home directory to a zip archive chosen by the user.
#[tauri::command]
pub fn export_config(app: AppHandle) -> Result<String, String> {
    use tauri_plugin_dialog::DialogExt;

    let home = dsh_home_path(&app)?;
    if !home.exists() {
        return Err(format!("DSH 数据目录不存在: {}", home.display()));
    }

    // Ask the user where to save the archive.
    let path = app
        .dialog()
        .file()
        .add_filter("Zip 压缩包", &["zip"])
        .set_file_name("dsh-config.zip")
        .blocking_save_file();
    let Some(path) = path else {
        return Err("已取消".to_string());
    };
    let dest = path.into_path().map_err(|e| e.to_string())?;

    // Zip the whole dsh-home directory into the destination file.
    let file = std::fs::File::create(&dest).map_err(|e| format!("创建文件失败: {e}"))?;
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();

    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(&home, &mut files)?;

    for f in &files {
        let rel = f.strip_prefix(&home).map_err(|e| e.to_string())?;
        let name = rel.to_string_lossy().replace('\\', "/");
        writer
            .start_file(name, options)
            .map_err(|e| format!("zip 写入失败: {e}"))?;
        let data = std::fs::read(f).map_err(|e| format!("读取文件失败 {}: {e}", f.display()))?;
        std::io::Write::write_all(&mut writer, &data).map_err(|e| e.to_string())?;
    }
    writer.finish().map_err(|e| format!("zip 完成失败: {e}"))?;

    Ok(format!("配置已导出到:\n{}", dest.display()))
}

/// Import a previously-exported zip archive into the DSH home directory.
#[tauri::command]
pub fn import_config(app: AppHandle) -> Result<String, String> {
    use tauri_plugin_dialog::DialogExt;

    let home = dsh_home_path(&app)?;

    // Ask the user to select the archive.
    let path = app
        .dialog()
        .file()
        .add_filter("Zip 压缩包", &["zip"])
        .blocking_pick_file();
    let Some(path) = path else {
        return Err("已取消".to_string());
    };
    let src = path.into_path().map_err(|e| e.to_string())?;

    // Ensure the target home dir exists.
    std::fs::create_dir_all(&home).map_err(|e| format!("创建目录失败: {e}"))?;

    let file = std::fs::File::open(&src).map_err(|e| format!("打开文件失败: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("读取 zip 失败: {e}"))?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("读取 zip 条目失败: {e}"))?;
        let name = entry.name().to_string();
        // Prevent path traversal.
        let out_path = home.join(&name);
        if !out_path.starts_with(&home) {
            return Err(format!("非法路径: {name}"));
        }
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path).map_err(|e| format!("创建目录失败: {e}"))?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
            }
            let mut out_file =
                std::fs::File::create(&out_path).map_err(|e| format!("创建文件失败: {e}"))?;
            std::io::copy(&mut entry, &mut out_file)
                .map_err(|e| format!("解压文件失败 {}: {e}", name))?;
        }
    }

    Ok(format!("配置已导入到:\n{}", home.display()))
}

/// Recursively collect all files under `dir` into `out`, skipping
/// `node_modules` directories (dependencies are rebuildable via pnpm).
fn collect_files(dir: &PathBuf, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("读取目录失败 {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "node_modules" || name == ".pnpm" {
            // Skip dependency directories (rebuildable, huge).
            continue;
        }
        if path.is_dir() {
            collect_files(&path, out)?;
        } else if path.is_file() {
            out.push(path);
        }
    }
    Ok(())
}
