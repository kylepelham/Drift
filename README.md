<p align="center">
  <img src="docs/assets/logo.svg" alt="Drift" width="120" />
</p>

<h1 align="center">Drift</h1>

<p align="center">
  A standalone desktop coding agent for Windows.
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#features">Features</a> ·
  <a href="#development">Development</a> ·
  <a href="#releases">Releases</a>
</p>

---

Drift embeds the [OpenCode](https://github.com/sst/opencode) engine as a compiled sidecar and wraps it in a fast native shell. Install one app, point it at your project folders, and work across as many threads as you need.

No separate Node install. No CLI bootstrap. No external server process to manage.

The UI stays deliberately plain: a sidebar of workspaces and threads, one chat pane, and a composer.

## Install

Download the latest Windows x64 installer from [Releases](https://github.com/kylepelham/Drift/releases).

On first launch:

1. Add a workspace (any project directory)
2. Open **Settings → Providers** and connect a model provider
3. Start a thread and send a prompt

Anthropic, OpenAI, and the rest of the providers OpenCode supports work here, including OAuth where the provider offers it.

## Features

- **Workspace-first sidebar** — threads live under folders, with archive and restore
- **Full OpenCode engine** — build/plan agents, custom agents, MCP servers, permissions, plugins
- **Shared sessions** — Drift and the OpenCode CLI can use the same projects at once
- **Fast transcripts** — virtualized chat that stays smooth at thousands of messages
- **Command palette** — `Ctrl+K`, rebindable shortcuts, per-session model/agent/thinking prefs
- **Themes and localization** — expanded palettes, custom CSS/fonts, and multi-language UI
- **Thread controls** — fork stable context, fork full history, or `/spawn` independent sibling work
- **Prompt and agent editing** — inspect and override model-family system prompts plus agent behavior
- **Auto-updates** — signed installers with one-click updates from GitHub Releases

## Development

Requirements: [Bun](https://bun.sh) and a Rust toolchain (for the Tauri shell).

```bash
bun install
bun install --ignore-scripts --cwd engine/upstream
bun install --cwd engine/opencode
bun run build:engine
```

Then:

```bash
bun run dev          # engine + Vite UI (browser)
bun run typecheck
bun run test
bun run build:native # native window, no installer
bun run package      # NSIS installer
```

`bun run dev` serves the UI at `http://localhost:5180` against an engine on port `4096`.
The native window (`bunx tauri dev`) reuses that Vite server, so frontend edits hot-reload in both places.

### Project layout

| Path | Description |
| --- | --- |
| `src/` | SolidJS frontend (`engine/`, `state/`, `ui/`) |
| `src-tauri/` | Tauri v2 shell, sidecar lifecycle, SQLite store |
| `engine/upstream/` | Vendored OpenCode monorepo (read-only subtree) |
| `engine/opencode/` | Drift engine extensions (plugins + config) |
| `docs/` | Architecture, engine integration, theming, extensibility |

### Updating the vendored engine

```bash
git fetch --no-tags https://github.com/sst/opencode.git dev
git subtree merge --prefix engine/upstream FETCH_HEAD --squash
bun install --ignore-scripts --cwd engine/upstream
bun run build:engine
```

Fetch with `--no-tags`. A plain `git subtree pull` imports every OpenCode release tag, and Drift’s release workflow triggers on `v*` tags.

See `docs/engine.md` for integration details and gotchas.

## Releases

Pushing a tag like `v1.1.0`:

1. Builds the engine and app on Windows
2. Signs update artifacts
3. Publishes the installer, signature, and `latest.json` update manifest to GitHub Releases

Installed copies check that manifest on startup and offer one-click updates (**Settings → General** to disable). Updates only move forward; deleting a bad release retracts it for anyone who has not installed it yet.

## License

MIT © Kyle Pelham

Drift bundles a compiled copy of [OpenCode](https://github.com/sst/opencode) (also MIT). Its license ships with the app under `licenses/opencode-LICENSE.txt`.
