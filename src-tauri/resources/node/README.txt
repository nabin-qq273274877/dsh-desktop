This directory holds the bundled Node.js 22 LTS runtime.

It is intentionally empty in the repository. The GitHub Actions release
workflow downloads the official Node.js 22 LTS distribution and extracts it
here before packaging, so the built installer ships with its own Node and does
not require Node to be pre-installed on the target machine.

Expected layout (from an official node-vX.Y.Z distribution):

  Windows:  node.exe, node_modules/npm/bin/npx-cli.js, ...
  macOS:    bin/node, lib/node_modules/npm/bin/npx-cli.js, ...

The Rust launcher resolves the bundled node(.exe) and npx-cli.js relative to
this directory at runtime (see src-tauri/src/launcher.rs).
