// DeepSeek Harness Desktop tools window — plain script using Tauri's global API.
const __TAURI__ = window.__TAURI__;
const invoke = __TAURI__.core.invoke;
const listen = __TAURI__.event.listen;

const pages = {
  "install-plugin": document.getElementById("page-install-plugin"),
  "list-plugins": document.getElementById("page-list-plugins"),
  "dsh-version": document.getElementById("page-dsh-version"),
  "settings": document.getElementById("page-settings"),
  "about": document.getElementById("page-about"),
  "changelog": document.getElementById("page-changelog"),
};

function showPage(name) {
  for (const [key, el] of Object.entries(pages)) {
    if (el) el.hidden = key !== name;
  }
  if (name === "list-plugins") refreshPlugins();
  if (name === "about") loadAboutVersion();
  if (name === "dsh-version") loadDshVersion();
  if (name === "settings") loadSettings();
  if (name === "changelog") loadChangelog();
}

function setOutput(el, text, kind = "") {
  if (!el) return;
  el.textContent = text;
  el.className = "output" + (kind ? " " + kind : "");
}

async function loadDesktopVersion() {
  try {
    const v = await invoke("get_desktop_version");
    const el = document.getElementById("desktop-ver");
    if (el) el.textContent = "v" + v;
  } catch (e) {}
}

async function loadAboutVersion() {
  try {
    const v = await invoke("get_desktop_version");
    const el = document.getElementById("about-ver");
    if (el) el.textContent = v;
  } catch (e) {}
}

async function loadDshVersion() {
  const el = document.getElementById("dsh-version-out");
  setOutput(el, "正在获取 DeepSeek Harness 版本…");
  try {
    const v = await invoke("get_dsh_version");
    setOutput(el, "DeepSeek Harness 版本: " + v, "ok");
  } catch (e) {
    setOutput(el, "获取失败: " + e, "err");
  }
}

// Parse the `dsh plugin list` output into [{name, version}] entries.
// The output lists each plugin as "├── name@version" or "└── name@version".
function parsePluginList(out) {
  const plugins = [];
  for (const line of out.split("\n")) {
    // 包名可能含 scope (@scope/name)，版本是最后一个 @ 之后(以数字开头)。
    const m = line.match(/[├└]──\s+(.+?)@([\d][^\s]*)\s*$/);
    if (m) {
      plugins.push({ name: m[1], version: m[2] });
    }
  }
  return plugins;
}

async function refreshPlugins() {
  const listEl = document.getElementById("plugin-list");
  const rawEl = document.getElementById("plugin-list-raw");
  listEl.innerHTML = "";
  listEl.appendChild(textNode("正在读取已安装插件…"));

  try {
    const out = await invoke("list_plugins");
    rawEl.textContent = out;
    rawEl.hidden = false;

    const plugins = parsePluginList(out);
    listEl.innerHTML = "";
    if (plugins.length === 0) {
      listEl.appendChild(textNode("(未解析到插件,详见下方原始输出)"));
      return;
    }
    for (const p of plugins) {
      listEl.appendChild(buildPluginItem(p));
    }
  } catch (e) {
    listEl.innerHTML = "";
    const err = document.createElement("div");
    err.textContent = "读取失败: " + e;
    err.className = "output err";
    listEl.appendChild(err);
  }
}

function textNode(text) {
  const d = document.createElement("div");
  d.textContent = text;
  d.className = "plugin-item";
  return d;
}

function buildPluginItem(p) {
  const item = document.createElement("div");
  item.className = "plugin-item";

  const name = document.createElement("span");
  name.className = "name";
  name.textContent = p.name;
  const ver = document.createElement("span");
  ver.className = "ver";
  ver.textContent = "@" + p.version;

  const btn = document.createElement("button");
  btn.className = "btn btn-danger";
  btn.textContent = "卸载";
  btn.addEventListener("click", () => removePlugin(p.name));

  item.appendChild(name);
  item.appendChild(ver);
  item.appendChild(btn);
  return item;
}

async function removePlugin(name) {
  if (!window.confirm(`确定要卸载插件「${name}」吗?`)) return;
  const listEl = document.getElementById("plugin-list");
  listEl.innerHTML = "";
  listEl.appendChild(textNode(`正在卸载 ${name} …`));
  try {
    const res = await invoke("remove_plugin", { package: name });
    await refreshPlugins();
  } catch (e) {
    listEl.innerHTML = "";
    const err = document.createElement("div");
    err.textContent = "卸载失败: " + e;
    err.className = "output err";
    listEl.appendChild(err);
  }
}

// Install plugin
document.getElementById("btn-install")?.addEventListener("click", async () => {
  const name = document.getElementById("plugin-name")?.value.trim();
  const input = document.getElementById("plugin-name");
  const btn = document.getElementById("btn-install");
  const out = document.getElementById("install-output");
  if (!name) {
    setOutput(out, "请输入插件包名", "err");
    return;
  }
  // Disable input + button during the (possibly long) install so the user
  // cannot double-submit, and show a clear "working" state instead of a
  // frozen-looking window.
  input.disabled = true;
  btn.disabled = true;
  const originalText = btn.textContent;
  btn.textContent = "安装中…";
  setOutput(out, `正在安装 ${name} …(下载依赖可能较慢,请耐心等待,不要关闭窗口)`);
  try {
    const res = await invoke("install_plugin", { package: name });
    setOutput(out, res || "安装完成", "ok");
    // 安装成功后自动跳转到"已安装插件"页。
    showPage("list-plugins");
  } catch (e) {
    setOutput(out, "安装失败: " + e, "err");
  } finally {
    input.disabled = false;
    btn.disabled = false;
    btn.textContent = originalText;
  }
});

document.getElementById("btn-refresh")?.addEventListener("click", refreshPlugins);

// ---------- settings ----------
function loadSettings() {
  const status = document.getElementById("settings-status");
  if (status) status.textContent = "";
  invoke("get_settings")
    .then((s) => {
      document.querySelectorAll('input[name="launcher"]').forEach((r) => {
        r.checked = r.value === s.launcher;
      });
      document.querySelectorAll('input[name="channel"]').forEach((r) => {
        r.checked = r.value === s.version_channel;
      });
      document.querySelectorAll('input[name="close"]').forEach((r) => {
        r.checked = r.value === s.close_behavior;
      });
    })
    .catch((e) => {
      if (status) status.textContent = "读取设置失败: " + e;
    });
}

document.getElementById("btn-save-settings")?.addEventListener("click", async () => {
  const launcher = document.querySelector('input[name="launcher"]:checked')?.value;
  const channel = document.querySelector('input[name="channel"]:checked')?.value;
  const close = document.querySelector('input[name="close"]:checked')?.value;
  const status = document.getElementById("settings-status");
  if (status) status.textContent = "正在保存…";
  try {
    await invoke("update_settings", {
      launcher,
      versionChannel: channel,
      closeBehavior: close,
    });
    if (status) status.textContent = "已保存(关闭行为立即生效;启动选项与版本通道重启 DSH 后生效)";
  } catch (e) {
    if (status) status.textContent = "保存失败: " + e;
  }
});

// ---------- changelog ----------
function buildChangelogEntry(entry) {
  const div = document.createElement("div");
  div.className = "changelog-entry";
  const title = document.createElement("h3");
  title.textContent = "v" + entry.title;
  const ul = document.createElement("ul");
  for (const c of entry.changes) {
    const li = document.createElement("li");
    li.textContent = c;
    ul.appendChild(li);
  }
  div.appendChild(title);
  div.appendChild(ul);
  return div;
}

async function loadChangelog() {
  const el = document.getElementById("changelog-list");
  if (!el) return;
  el.innerHTML = "";
  el.appendChild(textNode("正在加载更新日志…"));
  try {
    const entries = await invoke("get_changelog");
    el.innerHTML = "";
    for (const entry of entries) {
      el.appendChild(buildChangelogEntry(entry));
    }
  } catch (e) {
    el.innerHTML = "";
    const err = document.createElement("div");
    err.textContent = "加载更新日志失败: " + e;
    err.className = "output err";
    el.appendChild(err);
  }
}

// ---------- update flow ----------
// When the backend opens the about page to start an update (user opted in from
// the startup prompt or the "new version" menu indicator), run the update here.
listen("about-trigger-update", async () => {
  showPage("about");
  await runUpdate();
});

async function runUpdate() {
  const out = document.getElementById("update-output");
  const checkBtn = document.getElementById("btn-check-update");
  // Disable the "检查更新" button while a download/install is in progress so
  // the user can't trigger a second update concurrently.
  if (checkBtn) checkBtn.disabled = true;
  setOutput(out, "正在下载并安装新版本…");
  try {
    await invoke("install_update");
    setOutput(out, "更新已安装,应用即将重启", "ok");
  } catch (e) {
    setOutput(out, "更新失败: " + e, "err");
  } finally {
    if (checkBtn) checkBtn.disabled = false;
  }
}

document.getElementById("btn-check-update")?.addEventListener("click", async () => {
  const out = document.getElementById("update-output");
  setOutput(out, "正在检查更新…");
  try {
    const latest = await invoke("check_update");
    if (latest) {
      setOutput(out, "发现新版本 v" + latest + ",正在下载并安装…", "ok");
      await runUpdate();
    } else {
      setOutput(out, "无可用更新,已是最新版本", "");
    }
  } catch (e) {
    // Any check failure is treated as "no update available".
    setOutput(out, "无可用更新,已是最新版本", "");
  }
});

// Listen for page navigation requests from the menu.
listen("tools-page", (event) => {
  showPage(event.payload);
});

// Default page. Prefer an explicit ?page=... query (used e.g. by the launcher
// when it surfaces the plugin list after a plugin conflict), falling back to the
// install-plugin page.
(function () {
  let initial = "install-plugin";
  try {
    const q = new URLSearchParams(window.location.search);
    const wanted = q.get("page");
    if (wanted && pages[wanted]) initial = wanted;
  } catch (e) {}
  showPage(initial);
})();
