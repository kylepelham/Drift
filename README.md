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

Drift is a desktop coding agent with its own engine, written in Rust and linked into the app.
One process handles the agent loop, providers, tools, MCP servers, permissions, project
navigation, persistent thread history, updates, and Windows integration.

**One installer. No separate runtime. No CLI bootstrap. No local server to manage.**

> [!IMPORTANT]
> Drift is an agent, not a sandbox. It can read files, modify code, and run commands with
> your user account's permissions. Open trusted workspaces and review permission requests.

## Features

| | |
| --- | --- |
| **Projects at a glance** | Organize threads under workspace folders, rename and personalize workspaces, and archive or restore work without losing history. |
| **A native agent engine** | Build, plan and orchestrator agents, custom agents and commands, skills, subagents in the foreground or background, permissions, project instructions, and undo for every edit, in one Rust engine. |
| **Long-session performance** | Navigate virtualized transcripts with thousands of messages, streamed reasoning, syntax-highlighted tools, and persistent tool disclosure state. |
| **Context control** | Fork stable context or complete history, undo and redo turns, steer an active session, or spawn an independent sibling thread. |
| **Provider flexibility** | Connect Anthropic and OpenAI (keys, or your Claude and ChatGPT subscriptions), xAI (SuperGrok), Google, Vertex, Amazon Bedrock, Z.ai, OpenRouter, LM Studio, Ollama, or any OpenAI-compatible server; choose model, mode (fast, ultrafast, flex) and thinking level per thread. |
| **Deep configuration** | Edit each model family's base prompt, built-in agent behavior, permission rules, language servers, formatters, checks, and project instructions. |
| **MCP management** | Add stdio, streamable HTTP or SSE servers by hand or from a registry of popular ones, sign in with OAuth, and choose per server whether read-only agents may use its tools. |
| **Errors after every edit** | Language servers on your PATH (rust-analyzer, TypeScript, Pyright, gopls, clangd) report what an edit broke straight back to the agent. |
| **A workspace you can tune** | Use the command palette, rebind shortcuts, select from eight themes, customize fonts and CSS, and choose from 18 interface languages. |
| **Desktop behavior** | Open files in your editor, receive configurable notifications, use native folder dialogs, and install authenticated updates from GitHub Releases. |
| **Trusted-LAN remote control** | Opt in to the complete Drift interface from a phone or browser while the engine stays private on loopback. |

Coming from opencode? On first launch Drift imports your opencode conversations, sign-ins,
MCP servers, instructions, agents, commands and skills once, in the background, and shows
what it brought over and what it left out.

## Install

1. Download the latest Windows x64 installer from
   [GitHub Releases](https://github.com/kylepelham/Drift/releases/latest).
2. Launch Drift and add a workspace directory.
3. Open **Settings > Providers** and connect a model provider.
4. Start a thread and send a prompt.

Drift does not include paid model access. Provider accounts, terms, and usage charges
still apply. Installed copies check the authenticated update manifest on startup; automatic
checks can be disabled under **Settings > General**.

See Drift's [privacy policy](PRIVACY.md) for details about network connections and local data.

## How it works

```text
SolidJS interface
     | HTTP + one WebSocket, on loopback
     v
Drift engine (Rust, in process)  -->  model providers, MCP servers, language servers, project tools
     |
Tauri shell  --------------------->  SQLite (drift.db), updates, native Windows APIs
```

The engine listens only on `127.0.0.1` and requires a random token generated on each launch.
Model requests and relevant context go to whichever provider you configure. Drift plugins
run in the interface, from files you place in Drift's own config folder, so install
third-party plugins only when you trust their source. See the
[architecture](docs/architecture.md), [engine](docs/engine.md), [MCP](docs/mcp.md), and
[security policy](SECURITY.md) for details.

Optional [Remote Access](docs/remote.md) adds a Tauri-owned HTTPS gateway on port `41718` and
credential-free address discovery on UDP `41717`. It is disabled by default. Devices join by
entering a one-time code on the desktop (or an optional password), and traffic is encrypted
with a certificate authority that this computer creates and constrains to private addresses.

## Development

Drift's native target is Windows x64. Development requires:

- [Bun](https://bun.sh)
- A [stable Rust toolchain](https://rustup.rs/) with the MSVC target
- [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
  with **Desktop development with C++**
- [Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)

Clone the repository and install the frontend's dependencies:

```bash
git clone https://github.com/kylepelham/Drift.git
cd Drift
bun install
```

Start a headless engine and the browser UI:

```bash
bun run dev
```

The UI is served at `http://localhost:5180` against a headless engine (`drift-engined`) on a
scratch data folder. For the native window instead, run `bun run dev:shell`; it keeps its data
apart from an installed Drift.

### Quality checks

```bash
bun run gates
```

That runs typecheck, the bun tests, the generated-client check, clippy and every Rust test.
`bun run bench:engine` measures engine start, prompt overhead and prompt size.

### Build targets

```bash
bun run build:native  # release executable, no installer
bun run package       # Windows NSIS installer
```

### Project layout

| Path | Purpose |
| --- | --- |
| `src/` | SolidJS frontend, engine client/store, application state, and UI |
| `crates/drift-engine/` | The engine: API, sessions, providers, tools, edits, MCP, language servers, config, permissions, storage |
| `crates/drift-engined/` | Headless engine binary for the browser dev loop, conformance tests and remote hosts |
| `crates/drift-migrate/` | One-time importer from opencode's storage and config |
| `src-tauri/` | Tauri shell: window, updater, remote access, Drift's own store, links the engine |
| `scripts/` | Development, release, benchmark, and client generation tooling |
| `tests/` | Frontend, integration, and engine conformance tests |
| `docs/` | Architecture and subsystem documentation |

Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a substantial pull request.

## Documentation

| Guide | Covers |
| --- | --- |
| [Architecture](docs/architecture.md) | Layer boundaries, state flow, workspaces, transcripts, and tool rendering |
| [Engine](docs/engine.md) | How the app hosts the engine, its API and events, data locations, and usage limits |
| [Engine design](docs/engine-rewrite.md) | Every engine decision in detail, with milestones and baselines |
| [Extensibility](docs/extensibility.md) | Drift plugins, hooks, tool renderers, slash commands, and spawned threads |
| [MCP](docs/mcp.md) | Servers, transports, sign-in, read-only trust, and the registry |
| [Drift store](docs/store.md) | SQLite schema, persistence, archive behavior, and workspace lifecycle |
| [Theming](docs/theming.md) | Design tokens, built-in themes, fonts, and custom CSS |
| [Voice](docs/voice.md) | Dictation engine choice, capture and socket lifecycle, and settings |
| [Remote Access](docs/remote.md) | Full web UI, safe LAN discovery, authentication, threat model, and device testing |
| [Privacy policy](PRIVACY.md) | Local data, network connections, and user control |

## Contributing

Bug reports, focused fixes, and well-scoped improvements are welcome. Start with the
[contribution guide](CONTRIBUTING.md), use the repository's issue forms, and review the
[Code of Conduct](CODE_OF_CONDUCT.md).

Report vulnerabilities through GitHub's
[private vulnerability reporting](https://github.com/kylepelham/Drift/security/advisories/new),
not a public issue. General support guidance is in [SUPPORT.md](SUPPORT.md).

## OpenCode

Drift began as a desktop client around [OpenCode](https://github.com/sst/opencode) and now
runs its own engine. Parts of its behavior follow OpenCode's, and it imports OpenCode's data
once, so OpenCode's MIT license ships with the app at `licenses/opencode-LICENSE.txt`.

## License

Drift is available under the [MIT License](LICENSE). Copyright (c) 2026 Kyle Pelham.
