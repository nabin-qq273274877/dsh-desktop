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
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
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

/// Set when DSH fails because a package can't be resolved at all (missing store
/// content, half-cleared cache, etc.) rather than a real plugin conflict. These
/// must NOT open the plugin list — uninstalling won't help — and should instead
/// point the user at the clear-cache / reinstall flow.
static PLUGIN_MISSING: AtomicBool = AtomicBool::new(false);

/// Launch-session generation. Bumped on every (re)start request so stale
/// supervisor threads from earlier sessions stop touching the child, the
/// events, or the readiness flag — only the newest session's supervisor may
/// drive the launch.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Budget of automatic restarts after DSH had been ready and then died. Reset
/// once the server stays healthy for 5 minutes, so a crash loop eventually
/// stops and defers to the manual "重试启动" button.
static AUTO_RESTARTS: AtomicU32 = AtomicU32::new(0);

/// Serializes kill → spawn sequences so two rapid `start_dsh` calls can never
/// interleave their child-process handoffs.
static LAUNCH_LOCK: Mutex<()> = Mutex::new(());

/// Maximum automatic re-spawn attempts while DSH is still booting (before the
/// first ready signal). A first boot downloads the full dependency tree, where
/// a single transient network error used to abort the launch for good.
const MAX_BOOT_RETRIES: u32 = 3;


/// Overall boot timeout — dependency downloads on a slow first boot can take
/// a while, but not forever.
const BOOT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1200);

/// Iterations (× 800ms ≈ 3 min) the root page must serve 200 with the child
/// alive before the legacy readiness fallback fires (DSH builds without the
/// `/api` probe).
const LEGACY_READY_ITERATIONS: u32 = 225;

/// Watchdog iterations (× 5s = 60s) a live-but-unresponsive server is given
/// before it is treated as broken and restarted.
const WATCHDOG_UNHEALTHY_LIMIT: u32 = 12;

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
            return Ok(strip_extended_length_path(c));
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

/// Strip the Windows extended-length path prefix (`\\?\`) so the path can be
/// passed to child processes that don't tolerate it. Node v22.23.2's
/// `realpathSync` regresses on `\\?\`-prefixed paths (EISDIR on the drive
/// root when resolving the main module), so the bundled node/pnpm/npx paths
/// must be plain. `\\?\UNC\server\share` → `\\server\share`. No-op on non-
/// Windows or already-plain paths.
fn strip_extended_length_path(p: &PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let s = p.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{}", rest));
        }
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = p;
    }
    p.clone()
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

/// Idempotently create the whole `<app-data>/dsh-desktop` layout (`dsh-home`,
/// `store`, `cache`) and return the `dsh-desktop` directory.
///
/// Called from app setup (before any window logic depends on it) and from
/// every launch/subcommand path, so a *first* boot never races directory
/// initialization against the pnpm/DSH child. Unlike the previous inline
/// `let _ = create_dir_all(...)` calls, errors propagate: a missing/locked
/// data dir is the root cause of many "random" first-boot pnpm ENOENT
/// crashes and must surface loudly instead of killing the child silently.
pub(crate) fn ensure_data_dirs(app: &AppHandle) -> Result<PathBuf, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("failed to resolve app data dir: {e}"))?;
    // Strip the `\\?\` prefix so paths passed to node/pnpm via env vars
    // (PNPM_HOME, DSH_HOME, XDG_*, …) don't trip Node v22.23.2's realpathSync
    // regression (see `strip_extended_length_path`).
    let data_dir = strip_extended_length_path(&data_dir);
    let dsh_dir = data_dir.join("dsh-desktop");
    for sub in ["dsh-home", "store", "cache"] {
        let dir = dsh_dir.join(sub);
        std::fs::create_dir_all(&dir).map_err(|e| {
            format!(
                "failed to create data dir {}: {e} (请检查磁盘权限/杀毒软件锁定)",
                dir.display()
            )
        })?;
    }
    Ok(dsh_dir)
}

/// Build a `Command` that runs `@deepseek-ai/dsh <args>` through the configured
/// runner (`node pnpm.mjs dlx` or bundled `npx`), with the isolated
/// store/cache/DSH-home env, suitable for one-shot subcommands (`--version`,
/// `plugin --profile web add <pkg>`, ...). The version channel (dist-tag) is
/// taken from the current settings.
pub(crate) fn dsh_subcommand(app: &AppHandle, args: &[&str]) -> Result<Command, String> {
    let settings = crate::settings::get(app);
    let node_path = bundled_node_path(app)?;

    // Ensure the data dirs exist (pnpm lstat's `$HOME`/store; a missing dir on
    // macOS surfaces as `ENOENT: lstat .../com.dsh.desktop`). Unlike the old
    // `let _ = create_dir_all(...)` calls, failures propagate — a locked or
    // unwritable data dir is a real error the caller must see.
    let dsh_dir = ensure_data_dirs(app)?;
    let pnpm_store = dsh_dir.join("store");
    let dsh_home = dsh_dir.join("dsh-home");

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
        // Isolate pnpm's dlx cache (which otherwise lands in the user's global
        // %LOCALAPPDATA%\pnpm-cache\dlx) so clearing our store/cache also clears
        // the stale dlx entry that points at a deleted @deepseek-ai/dsh bin.js.
        .env("XDG_CACHE_HOME", &dsh_dir)
        .env("XDG_DATA_HOME", &dsh_dir)
        .env("XDG_STATE_HOME", &dsh_dir)
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
    append_log_file(app, &line);
    let _ = app.emit("dsh-log", line);
}

/// Append a line to the per-day log file under `<app-data>/dsh-desktop/logs/`,
/// named `YYYY-MM-DD.log` (local time), so each run's full DSH stdout/stderr is
/// persisted for offline troubleshooting. Failures are silent — logging must
/// never break the launch pipeline.
fn append_log_file(app: &AppHandle, line: &str) {
    use std::io::Write;
    let Ok(data_dir) = app.path().app_data_dir() else {
        return;
    };
    let logs_dir = data_dir.join("dsh-desktop").join("logs");
    if std::fs::create_dir_all(&logs_dir).is_err() {
        return;
    }
    let now = chrono::Local::now();
    let path = logs_dir.join(format!("{}.log", now.format("%Y-%m-%d")));
    let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(&path) else {
        return;
    };
    let _ = writeln!(f, "{} {}", now.format("%H:%M:%S"), line);
}

/// Append a diagnostic trace line to `<app-data>/dsh-desktop/launch-trace.log`
/// (mirrors `clear-trace.log`): lets field debugging see the launch-pipeline
/// steps even when no window is attached yet.
fn trace_launch(app: &AppHandle, msg: &str) {
    use std::io::Write;
    if let Ok(d) = app.path().app_data_dir() {
        let p = d.join("dsh-desktop").join("launch-trace.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(&p) {
            let _ = writeln!(f, "{msg}");
        }
    }
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

/// Spawn the DSH child process using the bundled Node binary: choose a fresh
/// port, ensure the data dirs exist, build the runner command, start it with
/// piped output, and stream its logs.
///
/// Readiness is NOT established here — the caller's supervisor thread probes
/// the web server and emits `dsh-ready` once the API layer is truly up (see
/// `probe`).
fn spawn_dsh(app: &AppHandle) -> Result<(), String> {
    trace_launch(app, "spawn_dsh: entry");
    // Fresh conflict detection for this process run: stale flags from a
    // previous (failed) run must not hijack this run's failure handling.
    PLUGIN_CONFLICT.store(false, Ordering::SeqCst);
    PLUGIN_MISSING.store(false, Ordering::SeqCst);

    let settings = crate::settings::get(app);
    let node_path = bundled_node_path(app)?;

    // Choose a free port so we never collide with a fixed port already in
    // use. (The port is later validated with a strong readiness probe, so the
    // small TOCTOU window between dropping this listener and DSH's bind can
    // never be mistaken for readiness, and an EADDRINUSE crash is retried on
    // a different port.)
    let port = find_free_port()?;
    *CURRENT_PORT.lock().unwrap() = Some(port);
    trace_launch(app, &format!("spawn_dsh: port {port} chosen"));

    // Use an isolated store/cache so we never touch the user's global pnpm/npm
    // (see dsh_subcommand). Creating the dirs here is what makes a *first*
    // boot work: pnpm lstat's `$HOME`/store and a missing dir aborts the
    // launch with ENOENT.
    let dsh_dir = ensure_data_dirs(app)?;
    let pnpm_store = dsh_dir.join("store");
    let dsh_home = dsh_dir.join("dsh-home");

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
        // Isolate pnpm's dlx cache (see dsh_subcommand) so clearing our cache
        // also clears the stale dlx entry that points at a deleted DSH bin.js.
        .env("XDG_CACHE_HOME", &dsh_dir)
        .env("XDG_DATA_HOME", &dsh_dir)
        .env("XDG_STATE_HOME", &dsh_dir)
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
        trace_launch(app, &format!("spawn_dsh: cmd.spawn FAILED: {e}"));
        emit_log(app, &format!("[error] failed to spawn DSH: {e}"));
        e.to_string()
    })?;
    trace_launch(app, &format!("spawn_dsh: child pid {}", child.id()));

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

    // A missing package is NOT a conflict: uninstalling a plugin won't help when
    // the store content itself is gone. Detect these first so they win over the
    // generic loader markers below.
    let missing_markers = [
        "err_module_not_found",
        "cannot find package",
        "cannot find module",
        "package not found",
        "enoent",
    ];
    if missing_markers.iter().any(|m| lower.contains(m)) {
        PLUGIN_MISSING.store(true, Ordering::SeqCst);
        return;
    }

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

/// 解析 `dsh plugin --profile web list` 输出,判断是否存在非系统插件。
///
/// 系统插件都发布在 `@deepseek-ai/` scope 下(dsh-*、cordis-*、cosmokit …)。
/// debug/next 通道加载系统插件时也会命中 `detect_plugin_conflict` 的 marker
/// (ESM 导出告警等),但卸载系统插件无用且危险;只有当列表里存在非
/// `@deepseek-ai/` 的第三方插件时,弹"已安装插件"页让用户卸载才有意义。
fn has_third_party_plugin(list_output: &str) -> bool {
    for line in list_output.lines() {
        let t = line.trim();
        let rest = t
            .strip_prefix("├── ")
            .or_else(|| t.strip_prefix("└── "))
            .unwrap_or("")
            .trim();
        if rest.is_empty() {
            continue;
        }
        // 形如 name@version,版本以数字开头;取最后一个 @(数字) 之前为 name。
        let name = match rest.rfind('@') {
            Some(i) if i + 1 < rest.len() && rest.as_bytes()[i + 1].is_ascii_digit() => &rest[..i],
            _ => continue,
        };
        if !name.starts_with("@deepseek-ai/") {
            return true;
        }
    }
    false
}

/// Outcome of one readiness probe against the DSH web server.
#[derive(PartialEq)]
enum Probe {
    /// The API transport route answered — DSH is fully ready.
    Ready,
    /// The server answered but the API layer is not up yet.
    NotReady,
    /// No answer at all (still booting / downloading dependencies).
    NoAnswer,
}

/// Probe DSH readiness with a request that proves the API layer is mounted.
///
/// A plain `GET /` returning 200 only proves the SPA fallback exists: the
/// webserver listens immediately while the `/api` prefix route (what the UI's
/// first "读取数据目录" fetch needs) is registered later by the
/// client-connection plugin, which answers `GET /api/events.mux` with
/// `426 Upgrade Required`. Requiring that exact status also rules out
/// mistaking a foreign local web server that grabbed our port for DSH — the
/// old `GET /` 2xx check did exactly that.
fn probe(client: &Option<reqwest::blocking::Client>, url: &str) -> Probe {
    let Some(client) = client else {
        return Probe::NoAnswer;
    };
    match client.get(format!("{url}/api/events.mux")).send() {
        Ok(resp) if resp.status().as_u16() == 426 => Probe::Ready,
        Ok(_) => Probe::NotReady,
        Err(_) => Probe::NoAnswer,
    }
}

/// Whether the DSH child process has exited.
fn child_exited() -> bool {
    CHILD
        .lock()
        .unwrap()
        .as_mut()
        .map(|c| matches!(c.try_wait(), Ok(Some(_))))
        .unwrap_or(false)
}

/// (Re)create the loading window outside the normal startup flow — used when
/// DSH dies or fails after the original loading window is already gone.
///
/// The window is created with `?autostart=0` so its JS does NOT invoke
/// `start_dsh` again (the supervisor already owns the restart); it shows the
/// log stream, offers the manual retry button, and destroys itself on
/// `dsh-ready` exactly like the startup window.
fn show_loading_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("loading") {
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        let url = tauri::WebviewUrl::App("index.html?autostart=0".into());
        let _ = tauri::WebviewWindowBuilder::new(&app2, "loading", url)
            .title("DeepSeek Harness Desktop")
            .inner_size(560.0, 420.0)
            .resizable(false)
            .maximizable(false)
            .center()
            .build();
    });
}

/// Supervise one launch session:
///
///   * phase 1 — wait for *strong* readiness (the `/api` route answering 426)
///     or child exit. Boot failures are re-spawned automatically with backoff:
///     a first boot downloads the whole dependency tree, where a single
///     transient network error used to kill the launch and require a manual
///     retry. Plugin conflicts / missing cache content are NOT retried (a
///     re-spawn cannot fix those) and surface their existing remediation UI.
///   * phase 2 — watchdog over the running server. If the child dies (e.g.
///     the fail-loud loader exits on a first-boot data-layer error) it is
///     restarted automatically within a bounded budget, the loading window is
///     re-shown so the user sees progress, and `emit_ready` re-navigates the
///     main window once the server is back — instead of leaving the user on a
///     dead page reporting "Failed to fetch". A live-but-unresponsive server
///     is restarted after a grace period.
///
/// The thread exits silently as soon as `GENERATION` moves past `gen` (a
/// newer start/restart request took over the session).
fn supervise(app: AppHandle, gen: u64) {
    trace_launch(&app, &format!("supervise: entry (gen {gen})"));
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .ok();

    let mut boot_retries: u32 = 0;

    'session: loop {
        // ---- phase 1: wait for the API layer to be ready (or the child to die) ----
        let mut attempts: u32 = 0;
        let mut root_ok_streak: u32 = 0;
        let boot_started = std::time::Instant::now();
        loop {
            if GENERATION.load(Ordering::SeqCst) != gen {
                return;
            }

            if child_exited() {
                // The child died before becoming ready.
                if PLUGIN_MISSING.load(Ordering::SeqCst) {
                    emit_log(&app, "[error] DSH 进程已退出,启动失败。");
                    emit_log(
                        &app,
                        "[hint] 检测到插件包缺失(缓存不完整)。请点击菜单「关于 → 清除 DSH 缓存」后重试,或重启应用重新安装。",
                    );
                    let _ = app.emit("dsh-launch-failed", ());
                    show_loading_window(&app);
                    return;
                }
                if PLUGIN_CONFLICT.load(Ordering::SeqCst) {
                    emit_log(&app, "[error] DSH 进程已退出,启动失败。");
                    // 先确认是否真有第三方插件可卸载。若 list 全是系统插件
                    // (@deepseek-ai/*),说明是 DSH 包自身(常见于 debug/next
                    // 通道)加载异常,卸载系统插件无用且危险——改走清缓存/
                    // 换通道引导,而不是打开"已安装插件"页让用户误卸载。
                    let has_third_party = match run_dsh_command(
                        &app,
                        &["plugin", "--profile", "web", "list"],
                    ) {
                        Ok(out) => has_third_party_plugin(&out),
                        Err(_) => false,
                    };
                    if has_third_party {
                        emit_log(
                            &app,
                            "[hint] 检测到插件冲突。已为你打开「已安装插件」页面,请卸载有问题的第三方插件后点击「重试启动」。",
                        );
                        let _ = app.emit("dsh-launch-failed", ());
                        crate::menu::open_plugin_list(&app);
                        show_loading_window(&app);
                    } else {
                        emit_log(
                            &app,
                            "[hint] 日志中出现插件加载告警,但未检测到第三方插件。这通常是 DSH 包(当前版本通道)自身加载异常或缓存损坏,而非插件冲突。请点击菜单「关于 → 清除 DSH 缓存」后重试,或在「设置」中切换版本通道(如切回 latest)。",
                        );
                        let _ = app.emit("dsh-launch-failed", ());
                        show_loading_window(&app);
                    }
                    return;
                }
                if boot_retries < MAX_BOOT_RETRIES {
                    boot_retries += 1;
                    let delay = 5 * boot_retries as u64;
                    emit_log(
                        &app,
                        &format!(
                            "[warn] DSH 启动过程中退出,{delay} 秒后自动重试(第 {boot_retries}/{MAX_BOOT_RETRIES} 次)…"
                        ),
                    );
                    std::thread::sleep(std::time::Duration::from_secs(delay));
                    if GENERATION.load(Ordering::SeqCst) != gen {
                        return;
                    }
                    match spawn_dsh(&app) {
                        Ok(()) => continue 'session,
                        Err(e) => {
                            emit_log(&app, &format!("[error] failed to spawn DSH: {e}"));
                            let _ = app.emit("dsh-launch-failed", ());
                            show_loading_window(&app);
                            return;
                        }
                    }
                }
                emit_log(&app, "[error] DSH 进程已退出,自动重试均失败。可点击下方「重试启动」再次尝试。");
                let _ = app.emit("dsh-launch-failed", ());
                show_loading_window(&app);
                return;
            }

            attempts += 1;
            std::thread::sleep(std::time::Duration::from_millis(800));
            if GENERATION.load(Ordering::SeqCst) != gen {
                return;
            }

            let port = CURRENT_PORT.lock().unwrap().unwrap_or(0);
            let url = dsh_url(port);

            match probe(&client, &url) {
                Probe::Ready => {
                    READY.store(true, Ordering::SeqCst);
                    emit_log(&app, &format!("[ready] DSH web server is up at {url}"));
                    emit_ready(&app);
                    break; // → phase 2
                }
                Probe::NotReady => {
                    root_ok_streak = 0;
                }
                Probe::NoAnswer => {
                    // Legacy fallback for DSH builds without the /api probe:
                    // accept a long, stable 200 on the root page while the
                    // child stays alive, so older versions still boot.
                    let root_ok = client
                        .as_ref()
                        .map(|c| {
                            c.get(&url)
                                .send()
                                .map(|r| r.status().is_success())
                                .unwrap_or(false)
                        })
                        .unwrap_or(false);
                    if root_ok {
                        root_ok_streak += 1;
                        if root_ok_streak >= LEGACY_READY_ITERATIONS {
                            emit_log(
                                &app,
                                "[warn] 未能探测到 /api 就绪信号,按旧版兼容方式判定就绪。",
                            );
                            READY.store(true, Ordering::SeqCst);
                            emit_ready(&app);
                            break;
                        }
                    } else {
                        root_ok_streak = 0;
                    }
                }
            }

            // Faint heartbeat so the user knows the app is still working
            // (dependency downloads may take a while); emitted rarely so a
            // real error is never pushed off-screen.
            if attempts == 5 {
                emit_log(
                    &app,
                    &format!("[waiting] {url} not ready yet,正在下载依赖,请稍候…"),
                );
            } else if attempts % 50 == 0 {
                emit_log(&app, &format!("[waiting] {url} 仍在等待,请耐心等待…"));
            }

            if boot_started.elapsed() > BOOT_TIMEOUT {
                emit_log(
                    &app,
                    "[error] 启动超时(20 分钟),已停止等待。可点击下方「重试启动」再次尝试。",
                );
                let _ = app.emit("dsh-launch-failed", ());
                show_loading_window(&app);
                return;
            }
        }

        // ---- phase 2: watchdog over the running server ----
        let mut healthy_secs: u32 = 0;
        let mut unhealthy_streak: u32 = 0;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            if GENERATION.load(Ordering::SeqCst) != gen {
                return;
            }

            let dead = child_exited();
            let port = CURRENT_PORT.lock().unwrap().unwrap_or(0);
            let unhealthy = dead || probe(&client, &dsh_url(port)) != Probe::Ready;

            if !unhealthy {
                healthy_secs += 5;
                unhealthy_streak = 0;
                // A server that stays healthy for 5 minutes re-arms the
                // auto-restart budget.
                if healthy_secs >= 300 {
                    AUTO_RESTARTS.store(0, Ordering::SeqCst);
                    healthy_secs = 0;
                }
                continue;
            }

            if !dead {
                unhealthy_streak += 1;
                if unhealthy_streak < WATCHDOG_UNHEALTHY_LIMIT {
                    if unhealthy_streak == 1 {
                        emit_log(&app, "[warn] DSH 服务暂时无响应,正在观察…");
                    }
                    continue;
                }
                emit_log(&app, "[warn] DSH 服务持续无响应,视为异常。");
            } else {
                emit_log(&app, "[error] DSH 进程意外退出。");
            }

            // Do NOT auto-restart once the main window is up: restarting would
            // navigate the main window away from DSH's own error page, hiding the
            // very failure the user needs to see. Keep the main window as-is and
            // stop supervising; the user can restart the app to retry.
            READY.store(false, Ordering::SeqCst);
            emit_log(
                &app,
                "[error] 已停止自动重启以保留 DSH 错误页面。请根据主窗口中的错误信息排查,或重启应用重新尝试。",
            );
            let _ = app.emit("dsh-launch-failed", ());
            return;
        }
    }
}

/// Kill the DSH child process (and its descendants) if still running.
///
/// This is intentionally non-blocking: it must be callable from window-close
/// and exit handlers without freezing the UI. The process tree is terminated
/// in the background via `taskkill /T /F`. Bumping the generation also stops
/// any supervisor thread still watching the old child.
pub fn kill_dsh() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
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
    // Once DSH is killed it is no longer ready; this lets a subsequent
    // start_dsh() actually restart it (e.g. after clearing the cache).
    READY.store(false, Ordering::SeqCst);
}

/// Kill the DSH child process tree and block until it has fully exited (or
/// `timeout` elapses), so file locks on the store/profile directories are
/// released before a new child is spawned. Also waits out a short settle
/// period for OS handle release.
///
/// Unlike `kill_dsh`, this does NOT bump the launch generation — callers
/// (start_dsh / clear_dsh_cache) manage the generation themselves so their
/// brand-new supervisor survives.
fn kill_and_wait(timeout: std::time::Duration) {
    let Some(mut child) = CHILD.lock().unwrap().take() else {
        return;
    };
    let pid = child.id();
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // Kill the whole process tree (DSH spawns cmd.exe / nested node).
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW: no console flash
            .spawn();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = child.kill();
    }
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    // Last-resort direct kill, then move on.
                    let _ = child.kill();
                    let _ = child.try_wait();
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(_) => break,
        }
    }
    // Give the OS a beat to release remaining file handles.
    std::thread::sleep(std::time::Duration::from_millis(300));
    *CURRENT_PORT.lock().unwrap() = None;
    READY.store(false, Ordering::SeqCst);
}

/// Tauri command: frontend can request a (re)start of the DSH launch flow.
///
/// The kill → wait-for-exit → spawn → supervise sequence runs on a background
/// thread (a sync command must not block the IPC caller for seconds), guarded
/// by `LAUNCH_LOCK` so concurrent invocations cannot interleave their
/// child-process handoffs. Failures are reported asynchronously via `dsh-log`
/// / `dsh-launch-failed`, matching how the loading window already handles
/// spawn errors.
#[tauri::command]
pub fn start_dsh(app: AppHandle) -> Result<(), String> {
    // A new session invalidates any supervisor still running for the old one.
    let gen = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    trace_launch(&app, &format!("=== start_dsh invoked (gen {gen})"));
    READY.store(false, Ordering::SeqCst);
    PLUGIN_CONFLICT.store(false, Ordering::SeqCst);
    PLUGIN_MISSING.store(false, Ordering::SeqCst);
    AUTO_RESTARTS.store(0, Ordering::SeqCst);

    std::thread::spawn(move || {
        let _guard = LAUNCH_LOCK.lock().unwrap();
        trace_launch(&app, "launch thread: acquired lock");
        // A newer session was requested while we waited for the lock.
        if GENERATION.load(Ordering::SeqCst) != gen {
            trace_launch(&app, "launch thread: superseded before kill_and_wait");
            return;
        }
        // Kill the old child and WAIT for it to exit: spawning immediately
        // after taskkill raced Windows file locks on the store/profile dirs
        // and made retries fail randomly.
        kill_and_wait(std::time::Duration::from_secs(5));
        if GENERATION.load(Ordering::SeqCst) != gen {
            trace_launch(&app, "launch thread: superseded after kill_and_wait");
            return;
        }
        if let Err(e) = spawn_dsh(&app) {
            trace_launch(&app, &format!("launch thread: spawn_dsh failed: {e}"));
            emit_log(&app, &format!("[error] failed to spawn DSH: {e}"));
            let _ = app.emit("dsh-launch-failed", ());
            return;
        }
        trace_launch(&app, "launch thread: spawn ok, starting supervisor");
        let app2 = app.clone();
        std::thread::spawn(move || supervise(app2, gen));
    });
    Ok(())
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

/// Payload for the `clear-progress` event.
#[derive(Clone, serde::Serialize)]
struct ClearProgress {
    pct: u32,
    label: String,
    done: bool,
}

/// Emit a progress update to the clear-cache loading window.
fn emit_clear_progress(app: &AppHandle, pct: u32, label: &str, done: bool) {
    let payload = ClearProgress {
        pct,
        label: label.to_string(),
        done,
    };
    // Global broadcast (same mechanism the tools window uses for `tools-page`,
    // which works reliably). The clear-loading window's JS listens for it.
    let _ = app.emit("clear-progress", payload);
}

/// Tauri command: clear DSH cache. `mode` is `"deps"` or `"all"`.
///
/// In BOTH modes user data is preserved (sessions, credentials, settings,
/// storages, attachments, ...). The two modes differ only in how much of the
/// plugin/dependency environment is removed:
///   * `"deps"` — remove only dependency caches (`profiles/web/node_modules`,
///     the pnpm `store`, and the download `cache`). Plugin config and installed
///     plugin declarations are kept; next launch re-downloads dependencies.
///   * `"all"`  — additionally remove the whole `profiles/web` profile (plugins
///     and their config are gone). User data under `dsh-home` is still kept.
///
/// DSH is killed first so no process holds file locks.
#[tauri::command]
pub fn clear_dsh_cache(app: AppHandle, mode: String) -> Result<String, String> {
    let mode = mode.trim().to_string();
    if mode != "deps" && mode != "all" {
        return Err("mode must be \"deps\" or \"all\"".to_string());
    }

    // Trace log to a file so we can diagnose progress/restart issues.
    let trace = |s: &str| {
        if let Ok(d) = app.path().app_data_dir() {
            let p = d.join("dsh-desktop").join("clear-trace.log");
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(&p) {
                let _ = writeln!(f, "{s}");
            }
        }
    };
    trace(&format!("=== clear start mode={mode}"));

    // Stop DSH first so files aren't locked. Bumping the generation stops any
    // running supervisor thread; then block until the process tree has really
    // exited instead of a fixed 1s sleep that raced Windows file-handle
    // release (and made the store deletion fail with a lock).
    GENERATION.fetch_add(1, Ordering::SeqCst);
    kill_and_wait(std::time::Duration::from_secs(5));
    emit_clear_progress(&app, 10, "正在停止 DSH 进程…", false);
    emit_log(&app, "[info] 正在清除 DSH 缓存,请稍候…");
    trace("killed dsh + waited");

    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("failed to resolve app data dir: {e}"))?;
    let dsh_dir = data_dir.join("dsh-desktop");
    let store = dsh_dir.join("store");
    let cache = dsh_dir.join("cache");
    let profile = dsh_dir.join("dsh-home").join("profiles").join("web");

    let mut removed: Vec<String> = Vec::new();
    let mut failed: Vec<String> = Vec::new();

    if mode == "all" {
        remove_dir(&profile, "插件环境 (profiles/web)", &mut removed, &mut failed);
        // Also remove the shared profile fallback node_modules (it can contain
        // real dirs like `typebox` that DSH expects to be symlinks — a leftover
        // from a previously corrupted profile).
        let shared_nm = dsh_dir.join("dsh-home").join("profiles").join("node_modules");
        remove_dir(&shared_nm, "共享模块目录 (profiles/node_modules)", &mut removed, &mut failed);
    } else {
        remove_dir(&profile.join("node_modules"), "插件依赖 (node_modules)", &mut removed, &mut failed);
    }
    emit_clear_progress(&app, 30, "正在删除依赖缓存(文件较多,请耐心)…", false);

    // Delete the (large) store synchronously and reliably (see remove_dir_fast
    // doc for why the old background-delete approach was removed).
    let removed_store = remove_dir_fast(&store, "pnpm 依赖缓存 (store)");
    emit_clear_progress(&app, 60, "已清除 pnpm 依赖缓存…", false);

    remove_dir(&cache, "下载缓存 (cache)", &mut removed, &mut failed);
    emit_clear_progress(&app, 75, "已清除下载缓存…", false);

    // Also clear pnpm's isolated dlx cache (XDG_CACHE_HOME/pnpm), which holds a
    // stale entry pointing at the deleted @deepseek-ai/dsh bin.js.
    remove_dir(&dsh_dir.join("pnpm"), "pnpm dlx 缓存", &mut removed, &mut failed);
    emit_clear_progress(&app, 85, "已清除 dlx 缓存…", false);

    // Surface a store deletion failure as a real error (instead of silently
    // treating it as "removed"), so the user knows the cache wasn't fully cleared.
    if removed_store.contains("失败") {
        failed.push(removed_store);
    } else if !removed_store.is_empty() {
        removed.push(removed_store);
    }

    trace(&format!("removed={removed:?} failed={failed:?}"));

    if !failed.is_empty() {
        emit_clear_progress(&app, 100, "清除失败", true);
        trace(&format!("FAILED: {}", failed.join(" | ")));
        return Err(format!(
            "部分清除失败:\n{}",
            failed.join("\n")
        ));
    }

    // Clear done. Restart by launching a fresh instance of the app, then exit
    // the current process (like the updater does). This avoids the flaky
    // in-process start_dsh restart.
    emit_clear_progress(&app, 100, "清除完成,正在重启应用…", true);
    trace("spawning fresh instance + exit");

    // Spawn a delayed launcher that starts a new app instance after this one
    // exits (releasing the single-instance mutex).
    let exe = std::env::current_exe().unwrap_or_default();
    let exe_str = exe.to_string_lossy().to_string();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("cmd")
            .args(["/c", "timeout", "/t", "2", "/nobreak", ">", "nul", "&", "start", "", &exe_str])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("sh")
            .args(["-c", &format!("sleep 2 && \"{}\" &", exe_str)])
            .spawn();
    }

    // Exit the current process so the single-instance mutex is released before
    // the new instance starts.
    std::process::exit(0);
}

/// Delete a directory (which may be large) reliably and synchronously.
///
/// Previous implementation renamed to a `.deleting` sibling and deleted it on a
/// background thread, then returned success immediately. That was broken in two
/// ways: (1) the background delete result was discarded, and (2) the process
/// often exited right after (via `std::process::exit`) before the delete ran,
/// leaving a leftover `store.deleting`. The leftover then split store content
/// across `store/` and `store.deleting/`, producing the "links exist but files
/// are missing" half-cleared state that surfaced as ERR_MODULE_NOT_FOUND.
///
/// We now delete synchronously with a short retry loop. Since DSH has already
/// been killed and we've waited a second for file handles to release, this is
/// reliable and only marginally slower.
fn remove_dir_fast(dir: &std::path::Path, label: &str) -> String {
    // Clean up any stale `.deleting` leftover from the old buggy implementation.
    let tmp = dir.with_extension("deleting");
    if tmp.exists() {
        let _ = remove_dir_with_retry(&tmp);
    }

    if !dir.exists() {
        return format!("{label}(不存在)");
    }

    // Try a direct delete first (handles release makes this usually succeed).
    if remove_dir_with_retry(dir) {
        return label.to_string();
    }

    // Fall back to rename-then-delete: rename is instant, then delete the
    // renamed dir synchronously (still on this thread, so it cannot be skipped
    // by a premature process exit).
    if std::fs::rename(dir, &tmp).is_ok() {
        if remove_dir_with_retry(&tmp) {
            return label.to_string();
        }
        // Rename succeeded but delete of the renamed dir failed — report it so
        // the caller surfaces a real error instead of silently leaving junk.
        return format!("{label}: 删除 {tmp:?} 失败");
    }

    format!("{label}: 删除失败")
}

/// Attempt `remove_dir_all` a few times, sleeping briefly between attempts to
/// let transient file locks clear. Returns true on success.
fn remove_dir_with_retry(dir: &std::path::Path) -> bool {
    for attempt in 0..3 {
        match std::fs::remove_dir_all(dir) {
            Ok(_) => return true,
            Err(_) if attempt < 2 => {
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
            Err(_) => return false,
        }
    }
    false
}

/// Recursively delete `dir`, recording the outcome into `removed`/`failed`.
fn remove_dir(
    dir: &std::path::Path,
    label: &str,
    removed: &mut Vec<String>,
    failed: &mut Vec<String>,
) {
    if !dir.exists() {
        removed.push(format!("{label}(不存在)"));
        return;
    }
    match std::fs::remove_dir_all(dir) {
        Ok(_) => removed.push(label.to_string()),
        Err(e) => failed.push(format!("{label}: {e}")),
    }
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
