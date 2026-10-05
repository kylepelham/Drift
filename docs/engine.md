# Engine

Drift's agent engine is a Rust library, `crates/drift-engine`, linked into the Tauri shell and
running in the app's own process. Every decision behind it, with its milestones and baselines,
is in [engine-rewrite.md](engine-rewrite.md); this page is the short version of how the app
uses it.

## How the app hosts it

- At setup `src-tauri/src/native.rs` opens the engine on `drift.db` in the app's data folder
  and serves its HTTP API and one WebSocket on `127.0.0.1`, on a free port, behind a random
  token made at each launch. The window asks the shell for both (`native_engine_status`); the
  engine never listens beyond loopback.
- Remote Access mounts the same router inside its HTTPS gateway, in process, and adds the
  token itself after a device signs in (see [remote.md](remote.md)).
- `crates/drift-engined` is the same engine headless: `bun run dev` runs it for the browser
  dev loop, the conformance tests drive it, and it can serve a remote host
  (`drift-engined [--data-dir DIR] [--port N]`, data in `~/.local/share/drift` by default).
- Settings the shell keeps (agent overrides from Settings > Agents, the shell time limit) are
  handed to the engine at startup and on every change; everything else the engine owns.

## Data

- One `drift.db`, one writer: the engine's tables beside the shell's own (workspaces,
  archives, preferences, remote devices). See [store.md](store.md).
- Sign-ins and API keys live in the operating system's credential store.
- The user's own config is `~/.config/drift`: `drift.json` (model, permission rules,
  providers, formatters, checks, language servers, skill paths), `AGENTS.md`, and `agents/`,
  `commands/` and `skills/`. A project adds its own `drift.json`, `.drift/{agents,commands,skills}`
  and `AGENTS.md` (or `CLAUDE.md`) files; it can never add providers or language servers.
- On first launch `crates/drift-migrate` imports opencode's conversations, sign-ins, MCP
  servers and config once, in the background ("Importing opencode conversations" in
  engine-rewrite.md).

## API and events

- The HTTP API is described by its OpenAPI document (`/openapi.json`); the UI's types in
  `src/engine/native/types.ts` are generated from it with `bun run gen:engine`, never written
  by hand. `src/engine/` is the only part of the UI that talks to the engine.
- Every event carries a monotonic `seq`. A client reconnects with `cursor`, the engine replays
  what it missed from a bounded window, and a client too far behind gets `resync` and hydrates
  again. A socket that falls behind while connected does the same.
- Questions the agent asks (async by default), permission asks and their replies, tasks and
  workers, undo and redo, compaction and retries are all engine features; engine-rewrite.md
  documents each.
## Context meter and plan usage limits

The context meter in the chat header shows the context window as one bar split by
category: system prompt and tool definitions, user messages, assistant replies, and tool
results. The total is the last reply's reported token count. The categories are estimated
at four characters per token from messages since the latest compaction summary. Whatever
that estimate leaves over is counted as system prompt and tools, which in practice is
mostly tool schemas.

Below the context window, the popover shows the plan limits of the provider behind the
current model, in the style of the Codex and Claude Code desktop apps. Each window has a
bar that turns amber at 70% and red at 90%. The ring in the header uses the same colors
for context usage.

`provider_usage` (`src-tauri/src/usage_limits.rs`) takes the credential from Drift's own
engine (`Engine::current_credential`, the keyring), calls the provider's usage endpoint, and
returns normalized windows (kind, optional label, percent used, and reset time in epoch
milliseconds). Tokens never reach the webview or a remote device. An expired sign-in is
renewed through the engine's own refresh, behind the same per-provider lock a turn uses, so
a rotating refresh token is never spent twice; when renewal fails the popover says the
sign-in has expired.
The frontend asks at most once a minute per provider, when the popover opens or a
session goes idle. **Settings > Usage limits** lists every linked provider that reports
limits, and its Refresh button bypasses the one-minute cache.

| Engine provider | Credential | Endpoint | Windows |
|---|---|---|---|
| `anthropic` | Claude Pro/Max OAuth | `api.anthropic.com/api/oauth/usage` | 5-hour, weekly, active per-model weekly caps |
| `openai` | ChatGPT OAuth | `chatgpt.com/backend-api/wham/usage` | classified by `limit_window_seconds` |
| `zai-coding-plan`, `zhipuai-coding-plan` | Coding Plan key | `/api/monitor/usage/quota/limit` | 5-hour and weekly token limits |
| `opencode-go` | Go key | `opencode.ai/zen/go/v1/usage` | rolling, weekly, monthly |
| `xai` | Grok OAuth | `cli-chat-proxy.grok.com/v1/billing?format=credits` | current credit period |
| `kimi-code-plan-global` | Kimi Code key | `api.kimi.com/coding/v1/usages` | 5-hour, weekly, monthly |
| `github-copilot` | GitHub OAuth | `api.github.com/copilot_internal/user` | monthly premium requests and chat |

These are private product endpoints, so response shapes can change without notice. The
Anthropic, OpenAI, and z.ai parsers were checked against live responses on 2026-09-28. The
other parsers follow CodexBar's source and fixtures. API keys for Anthropic and OpenAI
have no plan windows, so no section is shown for them. MiniMax is left out because its
wire units are unconfirmed, and Gemini because it has no suitable endpoint.
