// DSH Desktop tools window — plain script using Tauri's global API.
const __TAURI__ = window.__TAURI__;
const invoke = __TAURI__.core.invoke;
const listen = __TAURI__.event.listen;

const pages = {
  "install-plugin": document.getElementById("page-install-plugin"),
  "list-plugins": document.getElementById("page-list-plugins"),
  "check-update": document.getElementById("page-check-update"),
  "dsh-version": document.getElementById("page-dsh-version"),
  "about": document.getElementById("page-about"),
};

function showPage(name) {
  for (const [key, el] of Object.entries(pages)) {
    if (el) el.hidden = key !== name;
  }
  if (name === "list-plugins") refreshPlugins();
  if (name === "check-update") loadDesktopVersion();
  if (name === "about") loadAboutVersion();
  if (name === "dsh-version") loadDshVersion();
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
  setOutput(el, "正在获取 DSH 版本…");
  try {
    const v = await invoke("get_dsh_version");
    setOutput(el, "DSH 版本: " + v, "ok");
  } catch (e) {
    setOutput(el, "获取失败: " + e, "err");
  }
}

async function refreshPlugins() {
  const el = document.getElementById("plugin-list");
  setOutput(el, "正在读取已安装插件…");
  try {
    const out = await invoke("list_plugins");
    setOutput(el, out || "(无插件)", "");
  } catch (e) {
    setOutput(el, "读取失败: " + e, "err");
  }
}

// Install plugin
document.getElementById("btn-install")?.addEventListener("click", async () => {
  const name = document.getElementById("plugin-name")?.value.trim();
  const out = document.getElementById("install-output");
  if (!name) {
    setOutput(out, "请输入插件包名", "err");
    return;
  }
  setOutput(out, `正在安装 ${name} …(可能需要下载依赖,请稍候)`);
  try {
    const res = await invoke("install_plugin", { package: name });
    setOutput(out, res || "安装完成", "ok");
  } catch (e) {
    setOutput(out, "安装失败: " + e, "err");
  }
});

document.getElementById("btn-refresh")?.addEventListener("click", refreshPlugins);

document.getElementById("btn-check-update")?.addEventListener("click", async () => {
  const out = document.getElementById("update-output");
  setOutput(out, "正在检查更新…");
  try {
    const latest = await invoke("check_update");
    if (latest) {
      setOutput(out, "发现新版本 v" + latest + ",正在下载并安装…", "ok");
      try {
        await invoke("install_update");
        setOutput(out, "更新已安装,应用即将重启", "ok");
      } catch (e) {
        setOutput(out, "更新失败: " + e, "err");
      }
    } else {
      setOutput(out, "无可用更新,已是最新版本", "");
    }
  } catch (e) {
    setOutput(out, "检查更新失败: " + e, "err");
  }
});

// Listen for page navigation requests from the menu.
listen("tools-page", (event) => {
  showPage(event.payload);
});

// Default page.
showPage("install-plugin");
