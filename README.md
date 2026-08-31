<div align="center">

# DSH Desktop

**DeepSeek Harness 桌面启动器 · Desktop Launcher for DeepSeek Harness**

[**中文**](#中文) · [**English**](#english)

</div>

---

<a name="中文"></a>
## 中文

一个跨平台的 [DeepSeek Harness (DSH)](https://github.com/deepseek-ai/DeepSeek-Harness) 桌面启动器,支持 **Windows** 和 **macOS**。

它将 DSH 封装成原生桌面客户端,提供自定义启动页、实时启动日志、内嵌 Web 界面和自动更新——**目标机器无需预装 Node.js**。

### 特性

- **零依赖运行时** —— 内置 Node.js 22 LTS,用户无需安装 Node/npm。
- **无命令行窗口** —— DSH 在后台启动,stdout/stderr 实时流入启动页的日志框。
- **内嵌客户端** —— 就绪后,DSH 的 Web 界面加载到应用主窗口(启动页 → 主窗口)。
- **对 DSH 无侵入** —— 每次启动都通过 `pnpm dlx @deepseek-ai/dsh` 拉取最新版,DSH 更新无需改动本项目。
- **自动更新** —— `tauri-plugin-updater` 检查 GitHub Releases。
- **CI 打包** —— GitHub Actions 在版本 tag 上自动产出 Windows (NSIS) 和 macOS (dmg) 安装包。

### 启动命令

实际执行的内置 Node + pnpm 命令:

```
pnpm dlx @deepseek-ai/dsh web --port <随机端口> --no-open
```

端口在运行时自动选择(绑定 `127.0.0.1` 的 OS 分配空闲端口),因此固定端口冲突不会阻塞启动。就绪后,DSH Web 界面直接导航到主窗口。

内置 Node(22.13.0 LTS)通过 `pnpm dlx` 运行内置 pnpm(`pnpm/bin/pnpm.mjs`),使用硬链接缓存安装,比 `npx` 快得多。为加速首次安装,使用 npmmirror 镜像(`https://registry.npmmirror.com`)。所有 pnpm 状态隔离在单一用户数据目录下,不污染系统 pnpm/npm:

```
<app-data-dir>/dsh-desktop/
├── cache/    # pnpm 缓存
├── store/    # pnpm 内容寻址存储(硬链接)
└── dsh-home/ # DSH 独立数据目录(配置/插件/session)
```

### 构建要求

- Rust stable 工具链(rustup)
- MSVC Build Tools(Windows)/ Xcode CLT(macOS)
- Node 22(仅用于安装前端依赖 `@tauri-apps/api`)
- WebView2 运行时(Windows;Win10/11 已预装)

### 开发

```bash
npm install                 # 安装前端依赖
cargo tauri dev             # 开发模式运行
```

### 构建

```bash
cargo tauri build           # 产出 release 安装包
```

### 内置 Node 运行时

将 Node.js 22 LTS 发行版放到:

```
src-tauri/resources/node/
```

- **Windows**: `node/node.exe`、`node/node_modules/npm/bin/npx-cli.js`、...
- **macOS**: `node/bin/node`、`node/lib/node_modules/npm/bin/npx-cli.js`、...

并将 pnpm 放在其旁(通过 `node pnpm/bin/pnpm.mjs` 运行):

```
src-tauri/resources/node/pnpm/
├── bin/pnpm.mjs
├── dist/          # 内置 pnpm 运行时
└── package.json
```

> 启动器相对资源目录解析内置 `node(.exe)` 和 `pnpm/bin/pnpm.mjs`。GitHub Actions 工作流会自动下载两者。

### 自动更新配置

发布前,在 `src-tauri/tauri.conf.json` 中设置:

1. `plugins.updater.endpoints` → 你的 GitHub 仓库的 `latest.json` URL(替换 `<OWNER>` / `<REPO>`)。
2. `plugins.updater.pubkey` → `tauri signer generate` 生成的公钥。私钥需作为 `TAURI_SIGNING_PRIVATE_KEY` secret 存入 GitHub 仓库。

打一个 `vX.Y.Z` 的 tag 并 push,工作流会自动构建并发布安装包和更新清单。

### 许可证

[MIT](./LICENSE)

---

<a name="english"></a>
## English

A cross-platform desktop launcher for [DeepSeek Harness (DSH)](https://github.com/deepseek-ai/DeepSeek-Harness), supporting **Windows** and **macOS**.

It wraps DSH in a native desktop client with a custom loading screen, live startup logs, an embedded web UI, and automatic updates — **without requiring Node.js to be pre-installed**.

### Features

- **Zero-dependency runtime** — bundles Node.js 22 LTS; users do not need Node/npm installed.
- **No console window** — DSH is spawned in the background; stdout/stderr streams live into the loading screen's log box.
- **Embedded client** — once ready, DSH's web UI loads in the app's main window (loading screen → main window).
- **Non-invasive to DSH** — launches the latest DSH via `pnpm dlx @deepseek-ai/dsh`, so DSH updates never require a change to this project.
- **Automatic updates** — `tauri-plugin-updater` checks GitHub Releases.
- **CI builds** — GitHub Actions produces Windows (NSIS) and macOS (dmg) installers on every version tag.

### Launch command

The exact command run against the bundled Node + pnpm:

```
pnpm dlx @deepseek-ai/dsh web --port <free-port> --no-open
```

The port is chosen automatically at runtime (an OS-assigned free port bound to `127.0.0.1`). The DSH web UI is then navigated to directly in the app's main window.

The bundled Node (22.13.0 LTS) runs a bundled pnpm (`pnpm/bin/pnpm.mjs`) via `pnpm dlx`, giving hard-linked, cached installs — much faster than `npx`. The registry is npmmirror (`https://registry.npmmirror.com`). All pnpm state is isolated under a single per-user data directory:

```
<app-data-dir>/dsh-desktop/
├── cache/    # pnpm cache
├── store/    # pnpm content-addressable store (hard-linked)
└── dsh-home/ # isolated DSH data (config/plugins/sessions)
```

### Requirements (to build)

- Rust stable toolchain (rustup)
- MSVC Build Tools (Windows) / Xcode CLT (macOS)
- Node 22 (only for the `@tauri-apps/api` frontend dependency)
- WebView2 runtime (Windows; preinstalled on Win10/11)

### Development

```bash
npm install                 # install frontend deps
cargo tauri dev             # run in dev mode
```

### Build

```bash
cargo tauri build           # produce release installers
```

### Bundling the Node runtime

Place a Node.js 22 LTS distribution under:

```
src-tauri/resources/node/
```

- **Windows**: `node/node.exe`, `node/node_modules/npm/bin/npx-cli.js`, ...
- **macOS**: `node/bin/node`, `node/lib/node_modules/npm/bin/npx-cli.js`, ...

And pnpm beside it (run via `node pnpm/bin/pnpm.mjs`):

```
src-tauri/resources/node/pnpm/
├── bin/pnpm.mjs
├── dist/          # bundled pnpm runtime
└── package.json
```

> The launcher resolves the bundled `node(.exe)` and `pnpm/bin/pnpm.mjs` relative to the resource dir. The GitHub Actions workflow downloads both automatically.

### Auto-update configuration

Before releasing, set in `src-tauri/tauri.conf.json`:

1. `plugins.updater.endpoints` → your GitHub repo's `latest.json` URL (replace `<OWNER>` / `<REPO>`).
2. `plugins.updater.pubkey` → the public key from `tauri signer generate`. Store the private key as the `TAURI_SIGNING_PRIVATE_KEY` secret in your GitHub repository.

Tag a release with `vX.Y.Z` and push; the workflow builds and publishes the installers and update manifest automatically.

### License

[MIT](./LICENSE)
