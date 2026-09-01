// Clear-cache loading window — plain script using Tauri's global API.
const __TAURI__ = window.__TAURI__;
const listen = __TAURI__.event.listen;

const fill = document.getElementById("progress-fill");
const text = document.getElementById("progress-text");
const status = document.getElementById("clear-status");

function setProgress(pct, label) {
  if (fill) fill.style.width = pct + "%";
  if (text) text.textContent = pct + "%";
  if (status) status.textContent = label || "";
}

// Mark that JS actually loaded (so we can tell it apart from the static HTML).
if (status) status.textContent = "正在准备…";

// Backend emits `clear-progress` events: { pct, label, done }.
listen("clear-progress", (event) => {
  const p = event.payload || {};
  setProgress(p.pct || 0, p.label || "");
  if (p.done) {
    // Close this window once the clear + restart is finished.
    const win = __TAURI__.window.getCurrentWindow();
    win.destroy().catch(() => win.hide());
  }
});

// Initial state.
setProgress(0, "正在准备…");

