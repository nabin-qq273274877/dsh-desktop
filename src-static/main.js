// DSH Desktop frontend — plain script using Tauri's global API
// (enabled via `withGlobalTauri: true`), so no bundler is required.
const __TAURI__ = window.__TAURI__;
const invoke = __TAURI__.core.invoke;
const listen = __TAURI__.event.listen;
const win = __TAURI__.window.getCurrentWindow();

const isMain = win.label === "main";
const logEl = document.getElementById("log");

function appendLog(line, kind = "") {
  if (!logEl) return;
  const div = document.createElement("div");
  if (kind) div.className = kind;
  div.textContent = line;
  logEl.appendChild(div);
  logEl.scrollTop = logEl.scrollHeight;
}

function classify(line) {
  const lower = line.toLowerCase();
  if (/(error|failed|exception|eacces|enotfound|econnrefused)/.test(lower)) {
    return "line-err";
  }
  if (/(ready|listening|started|server.*up|compiled)/.test(lower)) {
    return "line-ok";
  }
  return "";
}

// ---------- loading window behavior ----------
if (!isMain) {
  appendLog("正在初始化…", "");

  // Show the real app version (from the Tauri app metadata).
  try {
    const ver = __TAURI__.app.getVersion();
    const el = document.getElementById("version-line");
    if (el && ver) el.textContent = `DSH Desktop v${ver}`;
  } catch (e) {
    // fall back to the static text already in the HTML
  }

  // Register log listeners before anything else so no lines are missed.
  listen("dsh-log", (event) => {
    appendLog(event.payload, classify(event.payload));
  });

  listen("dsh-ready", async (event) => {
    const url = event.payload || "http://127.0.0.1:3080";
    appendLog(`[ready] DSH 已就绪: ${url}`, "line-ok");
    appendLog("[ready] 正在打开主窗口…", "line-ok");
    try {
      await win.destroy();
    } catch (e) {
      await win.hide();
    }
  });

  // Flow: (1) check update -> (2) auto-install if found -> (3) else start DSH.
  async function boot() {
    appendLog("[update] 正在检查更新…", "");
    let latest = null;
    try {
      latest = await invoke("check_update");
    } catch (e) {
      appendLog(`[update] 检查更新失败(将直接启动): ${e}`, "");
    }

    if (latest) {
      appendLog(`[update] 发现新版本 v${latest},正在下载更新…`, "line-ok");
      appendLog("[update] 更新完成后应用会自动重启,请稍候…", "");
      try {
        await invoke("install_update");
        // On success the app relaunches; we may never reach here.
        appendLog("[update] 更新已安装,正在重启…", "line-ok");
        return;
      } catch (e) {
        appendLog(`[update] 更新失败(将直接启动当前版本): ${e}`, "line-err");
        // fall through to start DSH with the current version
      }
    } else {
      appendLog("[update] 已是最新版本", "");
    }

    await startDsh();
  }

  async function startDsh() {
    appendLog("正在启动 DSH…", "");
    try {
      await invoke("start_dsh");
    } catch (e) {
      appendLog(`[error] 启动 DSH 失败: ${e}`, "line-err");
    }
  }

  // Kick off the boot sequence once listeners are registered.
  boot();

  document.getElementById("btn-retry")?.addEventListener("click", async () => {
    if (logEl) logEl.innerHTML = "";
    await startDsh();
  });

  document.getElementById("btn-cancel")?.addEventListener("click", async () => {
    try {
      await invoke("plugin:window|exit");
    } catch (e) {
      await win.close();
    }
  });
}

// ---------- main window behavior ----------
// The backend navigates the main window directly to the DSH URL once ready.
if (isMain) {
  // (nothing to do)
}
