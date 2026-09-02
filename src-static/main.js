// DeepSeek Harness Desktop frontend — plain script using Tauri's global API
// (enabled via `withGlobalTauri: true`), so no bundler is required.
const __TAURI__ = window.__TAURI__;
const invoke = __TAURI__.core.invoke;
const listen = __TAURI__.event.listen;
const win = __TAURI__.window.getCurrentWindow();

const isMain = win.label === "main";
const logEl = document.getElementById("log");

// Once an error line has been appended, we stop auto-scrolling to the bottom so
// the real error (and its stack) stays visible instead of being pushed off by
// "waiting" heartbeat lines.
let stickToError = false;

function appendLog(line, kind = "") {
  if (!logEl) return;
  const div = document.createElement("div");
  if (kind) div.className = kind;
  div.textContent = line;
  logEl.appendChild(div);

  if (kind === "line-err" && !stickToError) {
    // Jump to the error so it's visible; afterwards keep it on screen.
    stickToError = true;
    logEl.scrollTop = div.offsetTop;
  } else if (!stickToError) {
    logEl.scrollTop = logEl.scrollHeight;
  }
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

  // The backend recreates this window with ?autostart=0 when DSH died after
  // being ready: a supervisor-owned restart is already in flight, so this
  // window must NOT call start_dsh again (it would kill and race the
  // in-flight restart).
  const autoStart = new URLSearchParams(window.location.search).get("autostart") !== "0";

  // Register log listeners before anything else so no lines are missed.
  listen("dsh-log", (event) => {
    appendLog(event.payload, classify(event.payload));
  });

  // Replay log lines the backend emitted before this window's JS listener was
  // ready (the backend buffers them for exactly this case — e.g. when the
  // loading window is recreated after DSH died mid-session).
  invoke("get_log_history")
    .then((lines) => {
      // Only the tail matters; keep the error lines visible.
      for (const line of lines.slice(-200)) appendLog(line, classify(line));
    })
    .catch(() => {});

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

  // DSH started OK but the child process later exited without becoming ready
  // (e.g. plugin/build failure). Enable the retry button so the user isn't
  // stuck with a disabled button.
  listen("dsh-launch-failed", () => {
    if (retryBtn) retryBtn.disabled = false;
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
  // "关于" menu. On a normal boot, start DSH immediately so loading is never
  // blocked; in a recovery window (?autostart=0) the supervisor already owns
  // the restart, so we only inform the user and keep the manual retry
  // button available.
  if (autoStart) {
    startDsh();
  } else {
    appendLog("DSH 服务异常退出,正在自动恢复…", "");
    if (retryBtn) retryBtn.disabled = false;
  }

  document.getElementById("btn-retry")?.addEventListener("click", async () => {
    if (logEl) logEl.innerHTML = "";
    stickToError = false;
    appendLog("正在重启 DSH…", "");
    await startDsh();
  });

  document.getElementById("btn-cancel")?.addEventListener("click", async () => {
    try {
      // Kill DSH and quit the whole app, so clicking 退出 really stops the
      // launcher instead of only closing the loading window (which would let
      // DSH keep starting in the background).
      await invoke("quit_app");
    } catch (e) {
      // Fallback: close the loading window.
      try {
        await invoke("plugin:window|exit");
      } catch (e2) {
        await win.close();
      }
    }
  });

  // "复制日志": copy whatever is currently shown in the log box to the clipboard.
  const copyBtn = document.getElementById("btn-copy-log");
  copyBtn?.addEventListener("click", async () => {
    if (!logEl) return;
    const text = logEl.innerText;
    const original = copyBtn.textContent;
    let copied = false;

    try {
      // Prefer the async Clipboard API (works in WebView2); fall back below.
      await navigator.clipboard.writeText(text);
      copied = true;
    } catch (e) {
      // Fallback for restricted contexts.
      try {
        const ta = document.createElement("textarea");
        ta.value = text;
        document.body.appendChild(ta);
        ta.select();
        copied = document.execCommand("copy");
        document.body.removeChild(ta);
      } catch (e2) {
        copied = false;
      }
    }

    copyBtn.textContent = copied ? "已复制" : "复制失败";
    copyBtn.disabled = true;
    setTimeout(() => {
      copyBtn.textContent = original;
      copyBtn.disabled = false;
    }, 1500);
  });
}

// ---------- main window behavior ----------
// The backend navigates the main window directly to the DSH URL once ready.
if (isMain) {
  // (nothing to do)
}
