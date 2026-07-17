# Drift

A lightweight desktop harness for the opencode engine. Codex-feel UI: one sidebar of
workspaces and threads, one chat pane, no tabs. See CHECKLIST.md for status and
docs/ for how it works.

## Requirements

- bun, and the `opencode` CLI on PATH (keep it current: `opencode upgrade`)
- Rust toolchain only if you build the Tauri shell

## Dev

```
bun install
bun run dev        # spawns opencode serve (port 4096) + vite on :5180
bun run typecheck
bun run build
```

Browser dev talks to the engine directly; if `OPENCODE_SERVER_PASSWORD` is set the dev
script forwards credentials automatically.

## Shell

```
bunx @tauri-apps/cli dev     # dev window (spawns its own engine on a random port)
bunx @tauri-apps/cli build   # release build
```
