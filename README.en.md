# DSH Desktop

**Desktop Launcher for DeepSeek Harness**

[中文](./README.md) · **English**

A cross-platform desktop launcher for [DeepSeek Harness (DSH)](https://github.com/deepseek-ai/DeepSeek-Harness), supporting **Windows** and **macOS**.

It wraps DSH in a native desktop client with a custom loading screen, live startup logs, an embedded web UI, and automatic updates — **without requiring Node.js to be pre-installed**.

## Features

- **Zero-dependency runtime** — bundles Node.js 22 LTS; users do not need Node/npm installed.
- **No console window** — DSH is spawned in the background; stdout/stderr streams live into the loading screen's log box.
- **Embedded client** — once ready, DSH's web UI loads in the app's main window (loading screen → main window).
- **Native menu** — a menu bar provides "Run" (install plugins), "View" (installed plugins), and "About" (version / check for updates).
- **Non-invasive to DSH** — launches the latest DSH via `pnpm dlx @deepseek-ai/dsh`, so DSH updates never require a change to this project.
- **Automatic updates** — `tauri-plugin-updater` checks GitHub Releases.
- **CI builds** — GitHub Actions produces Windows (NSIS) and macOS (dmg) installers on every version tag.

## Why not Electron

This project uses **Tauri 2.x** instead of Electron for these reasons:

| Aspect | Tauri 2.x | Electron |
|---|---|---|
| Memory usage | ~30–80 MB | ~150–250 MB |
| Installer size | ~30 MB (incl. bundled Node + pnpm) | ~100 MB+ (excl. bundled runtime) |
| Rendering engine | Reuses system WebView (WebView2 / WKWebView) | Bundles a full Chromium per app |
| Backend language | Rust | Node.js |
| Auto-update | `tauri-plugin-updater` (official) | `electron-updater` (mature) |
| CI packaging | `tauri-action` (official) | `electron-builder` (mature) |

For a **launcher**, the core needs are lightweight, fast startup, and low memory — not shipping a full browser. Tauri reuses the system's existing WebView, cutting footprint and memory to under a third of Electron, while still offering first-class official support for **auto-update** and **cross-platform CI packaging** — exactly what this project requires. The only trade-off is writing the backend in Rust, which is well worth it for a significantly smaller desktop launcher.

## Launch command

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

## Requirements (to build)

- Rust stable toolchain (rustup)
- MSVC Build Tools (Windows) / Xcode CLT (macOS)
- Node 22 (only for the `@tauri-apps/api` frontend dependency)
- WebView2 runtime (Windows; preinstalled on Win10/11)

## Development

```bash
npm install                 # install frontend deps
cargo tauri dev             # run in dev mode
```

## Build

```bash
cargo tauri build           # produce release installers
```

## Bundling the Node runtime

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

## Auto-update configuration

Before releasing, set in `src-tauri/tauri.conf.json`:

1. `plugins.updater.endpoints` → your GitHub repo's `latest.json` URL (replace `<OWNER>` / `<REPO>`).
2. `plugins.updater.pubkey` → the public key from `tauri signer generate`. Store the private key as the `TAURI_SIGNING_PRIVATE_KEY` secret in your GitHub repository.

Tag a release with `vX.Y.Z` and push; the workflow builds and publishes the installers and update manifest automatically.

## License

[MIT](./LICENSE)
