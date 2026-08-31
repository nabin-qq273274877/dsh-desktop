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

/// Resolve the DSH home data directory (`<app-data>/dsh-desktop/dsh-home`).
pub(crate) fn dsh_home_path(app: &AppHandle) -> Result<PathBuf, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("failed to resolve app data dir: {e}"))?;
    Ok(data_dir.join("dsh-desktop").join("dsh-home"))
}

/// Build a `Command` that runs `node pnpm.mjs dlx @deepseek-ai/dsh <args>`
/// with the isolated pnpm store/cache/DSH-home env, suitable for one-shot
/// subcommands (`--version`, `plugin --profile web add <pkg>`, ...).
pub(crate) fn dsh_subcommand(app: &AppHandle, args: &[&str]) -> Result<Command, String> {
    let node_path = bundled_node_path(app)?;
    let pnpm_path = bundled_pnpm_path(&node_path)?;

    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("failed to resolve app data dir: {e}"))?;
    let dsh_dir = data_dir.join("dsh-desktop");
    let pnpm_store = dsh_dir.join("store");
    let dsh_home = dsh_dir.join("dsh-home");

    let mut cmd = Command::new(&node_path);
    cmd.arg(&pnpm_path)
        .arg("--reporter=append-only")
        .arg("dlx")
        .arg("@deepseek-ai/dsh");
    for a in args {
        cmd.arg(a);
    }
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
        // Same network-resilience tuning as dsh_subcommand (see comments there).
        .env("npm_config_fetch_retries", "5")
        .env("npm_config_fetch_retry_mintimeout", "2000")
        .env("npm_config_fetch_retry_maxtimeout", "30000")
        .env("npm_config_fetch_timeout", "120000")
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
#[tauri::command]
pub async fn install_plugin(app: AppHandle, package: String) -> Result<String, String> {
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
            "add".into(),
            pkg,
        ],
    )
    .await
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
