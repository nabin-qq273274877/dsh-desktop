import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

const win = getCurrentWindow();

// A single HTML file renders both the loading window and the main window.
// The Rust backend sets which window we are in via the window label.
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

  // Replay any log lines emitted before our listener was ready, then subscribe
  // to new lines. This fixes the race where the backend starts DSH before the
  // frontend finishes loading.
  invoke("get_log_history")
    .then((history) => {
      for (const line of history) {
        appendLog(line, classify(line));
      }
    })
    .catch(() => {});

  // Stream logs from the Rust backend.
  listen("dsh-log", (event) => {
    appendLog(event.payload, classify(event.payload));
  });

  // When DSH is ready, the backend navigates & shows the main window directly.
  // We destroy the loading window here (not just hide) so it is fully released.
  listen("dsh-ready", async (event) => {
    const url = event.payload || "http://127.0.0.1:3080";
    appendLog(`[ready] DSH 已就绪: ${url}`, "line-ok");
    appendLog("[ready] 正在打开主窗口…", "line-ok");
    await win.destroy();
  });

  document.getElementById("btn-retry")?.addEventListener("click", async () => {
    if (logEl) logEl.innerHTML = "";
    appendLog("正在重启 DSH…", "");
    try {
      await invoke("start_dsh");
    } catch (e) {
      appendLog(`[error] ${e}`, "line-err");
    }
  });

  document.getElementById("btn-cancel")?.addEventListener("click", async () => {
    await invoke("plugin:window|exit");
  });
}

// ---------- main window behavior ----------
// The main window is a bare shell: the Rust backend navigates its webview
// directly to the DSH URL once DSH is ready, so no iframe is needed here.
if (isMain) {
  // (nothing to do; the backend handles navigation)
}
