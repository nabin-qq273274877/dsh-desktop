// Clear-cache loading window — plain script using Tauri's global API.
(function () {
  const status = document.getElementById("clear-status");
  const fill = document.getElementById("progress-fill");
  const text = document.getElementById("progress-text");

  function setProgress(pct, label) {
    if (fill) fill.style.width = pct + "%";
    if (text) text.textContent = pct + "%";
    if (status) status.textContent = label || "";
  }

  function fail(msg) {
    if (status) status.textContent = "错误: " + msg;
  }

  // Sanity: is the global Tauri API available?
  if (!window.__TAURI__) {
    fail("__TAURI__ 不可用");
    return;
  }
  if (!window.__TAURI__.event || !window.__TAURI__.event.listen) {
    fail("event.listen 不可用");
    return;
  }

  // Register the listener.
  window.__TAURI__.event
    .listen("clear-progress", (event) => {
      const p = event.payload || {};
      setProgress(p.pct || 0, p.label || "");
      if (p.done) {
        const win = window.__TAURI__.window.getCurrentWindow();
        win.destroy().catch(() => win.hide());
      }
    })
    .then(() => {
      // Listener registered successfully.
      setProgress(0, "已就绪,等待清除…");
    })
    .catch((e) => {
      fail("监听注册失败: " + e);
    });
})();
