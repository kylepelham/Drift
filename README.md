<p align="center">
  <img src="docs/assets/logo.svg" alt="Drift" width="112" />
</p>

<h1 align="center">Drift</h1>

<p align="center">
  <strong>A focused Windows desktop for coding with AI agents.</strong><br />
  Open your projects, keep long-running threads close, and let Drift manage the engine.
</p>

<p align="center">
  <a href="https://github.com/kylepelham/Drift/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/kylepelham/Drift?display_name=tag&sort=semver&style=flat-square" /></a>
  <a href="https://github.com/kylepelham/Drift/actions/workflows/ci.yml"><img alt="CI status" src="https://img.shields.io/github/actions/workflow/status/kylepelham/Drift/ci.yml?branch=master&style=flat-square&label=CI" /></a>
  <img alt="Windows x64" src="https://img.shields.io/badge/platform-Windows%20x64-2563eb?style=flat-square" />
  <a href="LICENSE"><img alt="MIT license" src="https://img.shields.io/github/license/kylepelham/Drift?style=flat-square" /></a>
</p>

<p align="center">
  <a href="#install">Install</a> &nbsp;|&nbsp;
  <a href="#features">Features</a> &nbsp;|&nbsp;
  <a href="#development">Development</a> &nbsp;|&nbsp;
  <a href="#documentation">Documentation</a>
</p>

---

Drift packages the [OpenCode](https://github.com/sst/opencode) agent engine as a
managed sidecar and gives it a native-feeling desktop home. One app handles the engine
lifecycle, project navigation, persistent thread metadata, updates, and Windows
integration while remaining compatible with OpenCode providers, agents, MCP servers,
plugins, and configuration.

**One installer. No separate Node runtime. No CLI bootstrap. No local server to manage.**

> [!IMPORTANT]
> Drift is an agent, not a sandbox. It can read files, modify code, and run commands with
> your user account's permissions. Open trusted workspaces and review permission requests.

## Features

| | |
| --- | --- |
| **Projects at a glance** | Organize threads under workspace folders, rename and personalize workspaces, and archive or restore work without losing engine sessions. |
| **The full OpenCode engine** | Use build and plan agents, custom agents, provider OAuth, MCP servers, permissions, project instructions, and plugins without installing OpenCode separately. |
| **Long-session performance** | Navigate virtualized transcripts with thousands of messages, streamed reasoning, syntax-highlighted tools, and persistent tool disclosure state. |
| **Context control** | Fork stable context or complete history, undo and redo turns, steer an active session, or spawn an independent sibling thread. |
| **Provider flexibility** | Connect Anthropic, OpenAI, GitHub Copilot, Google, OpenRouter, and the other providers supported by OpenCode; choose model, agent, and thinking settings per thread. |
| **Deep configuration** | Inspect and override model-family system prompts, built-in agent behavior, permissions, and project instructions. |
| **Guarded MCP management** | Review exact MCP server definitions before enabling them. Changed definitions require a new decision and invalid policy state fails closed. |
| **A workspace you can tune** | Use the command palette, rebind shortcuts, select from eight themes, customize fonts and CSS, and choose from 18 interface languages. |
| **Desktop behavior** | Open files in your editor, receive configurable notifications, use native folder dialogs, and install signed updates from GitHub Releases. |

Drift and the OpenCode CLI can use the same projects and canonical engine storage at the
same time. Drift-specific workspace names, icons, archive state, preferences, and MCP
decisions stay in Drift's own SQLite database.

## Install

1. Download the latest Windows x64 installer from
   [GitHub Releases](https://github.com/kylepelham/Drift/releases/latest).
2. Launch Drift and add a workspace directory.
3. Open **Settings > Providers** and connect a model provider.
4. Start a thread and send a prompt.

Drift does not include paid model access. Provider accounts, terms, and usage charges
still apply. Installed copies check the signed update manifest on startup; automatic
checks can be disabled under **Settings > General**.

See Drift's [privacy policy](PRIVACY.md) and [code signing policy](CODE_SIGNING.md) for
details about network connections, local data, and official Windows releases.

## How it works

```text
SolidJS interface
     | REST + server-sent events
     v
Bundled OpenCode sidecar  -------->  model providers, MCP servers, project tools
     ^
     | lifecycle + random loopback credentials
     |
Tauri shell  --------------------->  SQLite, updates, native Windows APIs
```

The sidecar listens only on `127.0.0.1` and receives a random Basic-auth credential on
each launch. Model requests and relevant context are sent to whichever provider you
configure. OpenCode and Drift plugins execute code in the engine process, so install
third-party plugins only when you trust their source. See the
[architecture](docs/architecture.md), [MCP trust boundary](docs/mcp.md), and
[security policy](SECURITY.md) for details.

## Development

Drift's native target is Windows x64. Development requires:

- [Bun](https://bun.sh)
- A [stable Rust toolchain](https://rustup.rs/) with the MSVC target
- [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
  with **Desktop development with C++**
- [Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)

Clone the repository and bootstrap the app plus the vendored engine:

```bash
git clone https://github.com/kylepelham/Drift.git
cd Drift
bun install
bun install --ignore-scripts --cwd engine/upstream
bun install --cwd engine/opencode
bun run build:engine
```

Start the engine and browser UI:

```bash
bun run dev
```

The UI is served at `http://localhost:5180` against the engine on port `4096`. To use the
native shell during development, leave that command running and start `bunx tauri dev`
in a second terminal.

### Quality checks

```bash
bun run typecheck
bun run test
cargo test --manifest-path src-tauri/Cargo.toml
```

Use `bun run test:engine` after changing engine overlays or extensions.

### Build targets

```bash
bun run build:native  # release executable, no installer
bun run package       # Windows NSIS installer
```

### Project layout

| Path | Purpose |
| --- | --- |
| `src/` | SolidJS frontend, engine client/store, application state, and UI |
| `src-tauri/` | Tauri shell, sidecar lifecycle, native commands, and Drift SQLite store |
| `engine/upstream/` | Pristine OpenCode git subtree; never edit directly |
| `engine/overlays/` | Minimal, reversible patches for internal engine integration points |
| `engine/opencode/` | Drift-shipped OpenCode plugins and configuration |
| `scripts/` | Development, extension, and engine build tooling |
| `tests/` | Focused frontend and integration tests |
| `docs/` | Architecture and subsystem documentation |

Read [CONTRIBUTING.md](CONTRIBUTING.md) before changing the vendored engine or opening a
substantial pull request.

## Documentation

| Guide | Covers |
| --- | --- |
| [Architecture](docs/architecture.md) | Layer boundaries, state flow, workspaces, transcripts, and tool rendering |
| [Engine integration](docs/engine.md) | Sidecar lifecycle, API surface, overlays, updates, and engine gotchas |
| [Extensibility](docs/extensibility.md) | OpenCode plugins, Drift hooks, tool renderers, and spawned threads |
| [MCP approvals](docs/mcp.md) | MCP trust boundary, exact-definition decisions, reloads, and recovery |
| [Drift store](docs/store.md) | SQLite schema, persistence, archive behavior, and workspace lifecycle |
| [Theming](docs/theming.md) | Design tokens, built-in themes, fonts, and custom CSS |
| [Privacy policy](PRIVACY.md) | Local data, network connections, and user control |
| [Code signing policy](CODE_SIGNING.md) | Release signing process and responsible roles |

## Contributing

Bug reports, focused fixes, and well-scoped improvements are welcome. Start with the
[contribution guide](CONTRIBUTING.md), use the repository's issue forms, and review the
[Code of Conduct](CODE_OF_CONDUCT.md).

Report vulnerabilities through GitHub's
[private vulnerability reporting](https://github.com/kylepelham/Drift/security/advisories/new),
not a public issue. General support guidance is in [SUPPORT.md](SUPPORT.md).

## OpenCode

Drift is an independent desktop client built around OpenCode. It bundles a compiled copy
of OpenCode under its MIT license and ships that license with the app at
`licenses/opencode-LICENSE.txt`.

## License

Drift is available under the [MIT License](LICENSE). Copyright (c) 2026 Kyle Pelham.
