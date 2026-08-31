# DSH Desktop

**DeepSeek Harness 桌面启动器**

**中文** · [English](./README.en.md)

一个跨平台的 [DeepSeek Harness (DSH)](https://github.com/deepseek-ai/DeepSeek-Harness) 桌面启动器,支持 **Windows** 和 **macOS**。

它将 DSH 封装成原生桌面客户端,提供自定义启动页、实时启动日志、内嵌 Web 界面和自动更新——**目标机器无需预装 Node.js**。

## 特性

- **零依赖运行时** —— 内置 Node.js 22 LTS,用户无需安装 Node/npm。
- **无命令行窗口** —— DSH 在后台启动,stdout/stderr 实时流入启动页的日志框。
- **内嵌客户端** —— 就绪后,DSH 的 Web 界面加载到应用主窗口(启动页 → 主窗口)。
- **原生菜单** —— 主窗口菜单栏提供「运行」(安装插件)、「查看」(已安装插件)、「关于」(版本 / 检查更新)。
- **对 DSH 无侵入** —— 每次启动都通过 `pnpm dlx @deepseek-ai/dsh` 拉取最新版,DSH 更新无需改动本项目。
- **自动更新** —— `tauri-plugin-updater` 检查 GitHub Releases。
- **CI 打包** —— GitHub Actions 在版本 tag 上自动产出 Windows (NSIS) 和 macOS (dmg) 安装包。

## 为什么不使用 Electron

本项目选用 **Tauri 2.x** 而非 Electron,主要基于以下考量:

| 维度 | Tauri 2.x | Electron |
|---|---|---|
| 内存占用 | ~30–80 MB | ~150–250 MB |
| 安装包体积 | ~30 MB(含内置 Node + pnpm) | ~100 MB+(不含内置运行时) |
| 渲染内核 | 复用系统 WebView(WebView2 / WKWebView) | 每个应用内置完整 Chromium |
| 后端语言 | Rust | Node.js |
| 自动更新 | `tauri-plugin-updater` 官方支持 | `electron-updater`(成熟) |
| CI 打包 | `tauri-action` 官方支持 | `electron-builder`(成熟) |

对一个「启动器」而言,核心诉求是**轻量、启动快、内存占用低**,而不是承载一个完整浏览器。Tauri 复用系统已有的 WebView,把体积和内存降到 Electron 的 1/3 以下,同时官方对**自动更新**和 **CI 双平台打包**都是一等支持,正好覆盖本项目的全部硬需求。唯一代价是后端需用 Rust,但换来的是显著更小的资源占用,对桌面启动器更合适。

## 启动命令

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

## 构建要求

- Rust stable 工具链(rustup)
- MSVC Build Tools(Windows)/ Xcode CLT(macOS)
- Node 22(仅用于安装前端依赖 `@tauri-apps/api`)
- WebView2 运行时(Windows;Win10/11 已预装)

## 开发

```bash
npm install                 # 安装前端依赖
cargo tauri dev             # 开发模式运行
```

## 构建

```bash
cargo tauri build           # 产出 release 安装包
```

## 内置 Node 运行时

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

## 自动更新配置

仓库: [nabin-qq273274877/dsh-desktop](https://github.com/nabin-qq273274877/dsh-desktop)。

发布前,在 `src-tauri/tauri.conf.json` 中已配置好:

1. `plugins.updater.endpoints` → `https://github.com/nabin-qq273274877/dsh-desktop/releases/latest/download/latest.json`
2. `plugins.updater.pubkey` → `tauri signer generate` 生成的公钥。私钥需作为 `TAURI_SIGNING_PRIVATE_KEY` secret 存入 GitHub 仓库。

打一个 `vX.Y.Z` 的 tag 并 push,工作流会自动构建并发布安装包和更新清单。

## 许可证

[MIT](./LICENSE)
