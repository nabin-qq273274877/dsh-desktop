import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";

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

  // Stream logs from the Rust backend.
  listen("dsh-log", (event) => {
    appendLog(event.payload, classify(event.payload));
  });

  // When DSH is ready, open the main window and hide the loading window.
  listen("dsh-ready", async (event) => {
    const url = event.payload || "http://127.0.0.1:3080";
    appendLog(`[ready] DSH 已就绪: ${url}`, "line-ok");
    appendLog("[ready] 正在打开主窗口…", "line-ok");

    const mainWin = WebviewWindow.getByLabel("main");
    if (mainWin) {
      await mainWin.show();
      await mainWin.setFocus();
    }
    await win.hide();
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
if (isMain) {
  // The main window renders an iframe pointed at the embedded DSH web app.
  // The URL is resolved from the backend (the dynamically chosen port).
  const frame = document.createElement("iframe");
  frame.className = "dsh-frame";
  frame.allow = "clipboard-read; clipboard-write";
  document.body.appendChild(frame);

  async function loadDsh() {
    try {
      const url = await invoke("get_dsh_url");
      if (url && url !== "http://127.0.0.1:0") {
        frame.src = url;
      }
    } catch (e) {
      // DSH not ready yet; retry shortly.
    }
  }

  loadDsh();

  // The port is fixed once chosen; re-check in case the main window opened
  // before the backend finished choosing the port.
  setTimeout(loadDsh, 500);
}
