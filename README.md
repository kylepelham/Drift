# Drift

A standalone desktop coding agent. Embeds the opencode engine (vendored source,
compiled into a bundled sidecar binary), fronted by a Codex-feel UI: one sidebar of
workspaces and threads, one chat pane, no tabs. Nothing to install besides Drift.
See CHECKLIST.md for status and docs/ for how it works.

## Requirements (development)

- bun
- Rust toolchain for the Tauri shell

## First-time setup

```
bun install
bun install --ignore-scripts --cwd engine/upstream
bun install --cwd engine/opencode   # deps for Drift's engine plugins
bun run build:engine                # compiles the embedded engine sidecar
```

## Dev

```
bun run dev        # embedded engine (port 4096) + vite on :5180, in the browser
bun run typecheck
bun run build
```

## Desktop app

```
bunx @tauri-apps/cli dev     # dev window, spawns its own engine on a random port
bunx @tauri-apps/cli build   # release build
```

## Updating the engine

```
git subtree pull --prefix engine/upstream opencode dev --squash
bun install --ignore-scripts --cwd engine/upstream
bun run build:engine
```
