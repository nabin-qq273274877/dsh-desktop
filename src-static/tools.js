// DSH Desktop tools window — plain script using Tauri's global API.
const __TAURI__ = window.__TAURI__;
const invoke = __TAURI__.core.invoke;
const listen = __TAURI__.event.listen;

const pages = {
  "install-plugin": document.getElementById("page-install-plugin"),
  "list-plugins": document.getElementById("page-list-plugins"),
  "dsh-version": document.getElementById("page-dsh-version"),
  "about": document.getElementById("page-about"),
};

function showPage(name) {
  for (const [key, el] of Object.entries(pages)) {
    if (el) el.hidden = key !== name;
  }
  if (name === "list-plugins") refreshPlugins();
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

// Parse the `dsh plugin list` output into [{name, version}] entries.
// The output lists each plugin as "├── name@version" or "└── name@version".
function parsePluginList(out) {
  const plugins = [];
  for (const line of out.split("\n")) {
    const m = line.match(/[├└]──\s+([^\s@]+)@(.+)/);
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
  const out = document.getElementById("install-output");
  if (!name) {
    setOutput(out, "请输入插件包名", "err");
    return;
  }
  setOutput(out, `正在安装 ${name} …(可能需要下载依赖,请稍候)`);
  try {
    const res = await invoke("install_plugin", { package: name });
    setOutput(out, res || "安装完成", "ok");
    // 安装成功后自动跳转到"已安装插件"页。
    showPage("list-plugins");
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
