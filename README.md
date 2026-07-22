# Drift

A standalone desktop coding agent for Windows.

Drift embeds the [opencode](https://github.com/sst/opencode) engine as a compiled
sidecar binary, wrapped in a fast native shell. Install one thing, point it at your
project folders, and work with an agent across as many threads as you need. There is
nothing else to set up: no Node install, no CLI, no separate server.

The UI is deliberately plain: a sidebar of workspaces and their threads, one chat
pane, and a composer. No tabs.

## Install

Grab the latest installer from [Releases](../../releases). Windows x64 only for now.

On first run, add a workspace (any directory), pick a model, and connect a provider
under Settings > Providers. Anthropic, OpenAI, and everything else opencode supports
works here, including OAuth sign-in where the provider offers it.

## Features

- Workspaces are directories. Threads live under them in the sidebar, with archive
  and restore (archived threads keep for 7 days).
- Full opencode engine underneath: build/plan agents, custom agents, MCP servers,
  permissions, plugins, todo tracking.
- Sessions are shared with opencode. Drift and the opencode CLI/desktop can run
  against the same projects at the same time.
- Virtualized transcript that stays smooth at thousands of messages.
- Command palette (Ctrl+K), rebindable keys, per-session model/agent/thinking
  preferences, light/dark/slate themes.
- Fork threads, spawn subagent threads, revert to any point in a conversation.

## Development

Requirements: [bun](https://bun.sh) and a Rust toolchain (for the Tauri shell).

```
bun install
bun install --ignore-scripts --cwd engine/upstream
bun install --cwd engine/opencode
bun run build:engine
```

Then:

```
bun run dev          # engine + vite dev server, runs in the browser
bun run typecheck
bun run test
bun run build:native # native window, no installer
bun run package      # NSIS installer
```

`bun run dev` serves the UI at `localhost:5180` against an engine on port 4096.
The native dev window (`bunx tauri dev`) reuses that vite server, so frontend edits
hot-reload in both.

### Project layout

| Path | What it is |
| --- | --- |
| `src/` | SolidJS frontend. `engine/` talks to the engine, `state/` is app state, `ui/` is components. |
| `src-tauri/` | Tauri v2 shell: sidecar lifecycle, SQLite store, native commands. |
| `engine/upstream/` | Pristine opencode monorepo, vendored via git subtree. Never edited. |
| `engine/opencode/` | Drift's engine extensions, applied as opencode plugins/config. |
| `docs/` | Architecture, engine integration, theming, extensibility. |

### Updating the vendored engine

```
git fetch --no-tags https://github.com/sst/opencode.git dev
git subtree merge --prefix engine/upstream FETCH_HEAD --squash
bun install --ignore-scripts --cwd engine/upstream
bun run build:engine
```

Fetch with `--no-tags`: a plain `git subtree pull` drags in every opencode release
tag, and this repo's release workflow triggers on `v*` tags.

`docs/engine.md` covers the gotchas.

## Releases

Pushing a tag like `v1.0.0` builds the app and publishes the installer to GitHub
Releases automatically.

## License

MIT. Drift bundles a compiled copy of [opencode](https://github.com/sst/opencode),
which is also MIT licensed; its license ships with the app under
`licenses/opencode-LICENSE.txt`.
