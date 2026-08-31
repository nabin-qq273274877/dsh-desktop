// DeepSeek Harness Desktop frontend — plain script using Tauri's global API
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

  const retryBtn = document.getElementById("btn-retry");
  // Retry button starts disabled; it is only enabled when startup fails.
  if (retryBtn) retryBtn.disabled = true;

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

  async function startDsh() {
    appendLog("正在启动 DSH…", "");
    if (retryBtn) retryBtn.disabled = true;
    try {
      await invoke("start_dsh");
      // Note: a successful spawn does not mean "ready" — readiness is signalled
      // asynchronously via the `dsh-ready` event.
    } catch (e) {
      appendLog(`[error] 启动 DSH 失败: ${e}`, "line-err");
      // Enable retry on failure.
      if (retryBtn) retryBtn.disabled = false;
    }
  }

  // No version check / update check here: updates are handled from the
  // "关于" menu. Just start DSH immediately so loading is never blocked.
  startDsh();

  document.getElementById("btn-retry")?.addEventListener("click", async () => {
    if (logEl) logEl.innerHTML = "";
    appendLog("正在重启 DSH…", "");
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
