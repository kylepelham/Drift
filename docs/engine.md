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
  With `--acp` it speaks the Agent Client Protocol on stdio instead, so an editor such as Zed
  drives Drift sessions ([acp.md](acp.md)).
- Settings the shell keeps (agent overrides from Settings > Prompts, the shell time limit) are
  handed to the engine at startup and on every change; everything else the engine owns.

## Data

- One `drift.db`, one writer: the engine's tables beside the shell's own (workspaces,
  archives, preferences, remote devices). See [store.md](store.md).
- Sign-ins and API keys live in the operating system's credential store. A provider can hold
  several sign-ins, used in order: a turn moves to the next the moment one reaches its
  five-hour or weekly limit ("A provider can hold several sign-ins" in engine-rewrite.md).
- The user's own config is `~/.config/drift`: `drift.json` (model, permission rules,
  providers, formatters, checks, language servers, skill paths), `AGENTS.md`, and `agents/`,
  `commands/` and `skills/`. A project adds its own `drift.json`, `.drift/{agents,commands,skills}`
  and `AGENTS.md` (or `CLAUDE.md`) files; it can never add providers or language servers.
- On first launch `crates/drift-migrate` imports opencode's conversations, sign-ins, MCP
  servers and config once, in the background, from opencode v1 and v2 alike ("Importing opencode
  conversations" in engine-rewrite.md).

Tool-call metadata uses `ToolMetadata` in `session/types.rs`, shared by tool results,
running progress, history capture and replay. Its OpenAPI schema gives the generated
client named fields without changing the JSON stored in `part.json` or sent over HTTP
and WebSocket. The tool-call variant boxes metadata to keep other part variants small.

Absent fields remain absent; explicit nulls such as an unlimited `shellTimeoutMs` or a
large read's unknown `lines` stay null. `files` accepts native path strings and imported
opencode file objects. `images` keeps returned `{mime, data}` entries and stored
`{mime, hash}` entries. Unknown keys, including `filediff`, stay in a flattened map.
Known fields with legacy shapes and non-object metadata are preserved rather than
making a stored tool call unreadable. No SQLite migration or Tauri IPC change is needed.

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

## References

`references` in drift.json names folders and git repositories outside the workspace that the
model may consult, such as another project's code, a spec, or a library's source:

```json
{
  "references": {
    "notes": "~/notes",
    "spec": { "path": "../wire-spec", "description": "The protocol spec" },
    "rust": { "repository": "https://github.com/rust-lang/rust", "branch": "master" }
  }
}
```

- A path is relative to the drift.json naming it, absolute, or starts with `~/`.
- A repository is cloned shallow under Drift's data directory (`references/<name>-<hash>`) the
  first time a turn's config names it, in the background, and fetched again at most once a day
  when a turn uses it. Drift never edits the clone; a fetch resets it to what was fetched.
- The instruction baseline lists each reference: its name, description and local path, and a note
  while a clone is still being made.
- Reading and searching inside one of your own references (from `~/.config/drift/drift.json`) runs
  without asking; files likely to hold secrets still ask, and writes ask as for any path outside the
  workspace. Shell commands that read there ask as any outside path does.
- A project's drift.json may name references too, but they grant nothing: reading in one outside the
  workspace asks like any outside path, and answering that ask (Always leaves the usual folder grant)
  is the approval. A project's repository reference is listed but never fetched, so a committed file
  can never make Drift fetch from the network. A project reference never replaces one of yours of
  the same name.

## Queued prompts

A prompt sent while a turn runs is delivered as `steer` by default: it joins the running turn at its
next request. Sent as `queue` (Alt+Enter in the composer, Send clicked with Alt held, or a long press on
Send on a touch screen, while a turn runs), it waits instead, kept outside the transcript (`queued_prompt` table) and shown above
the composer with Edit, Discard and Send now. Send now takes it off the queue and sends it as a
steer, or as a new turn when none runs.

- When a turn ends on its own, the first queued prompt starts the next turn; the one after it waits
  for that turn to end. Steers sent meanwhile are already part of the turn, so they land first.
- A queued prompt sent while nothing runs starts at once.
- After Stop the queue is held: nothing starts until the user sends a prompt, and when that turn
  ends the queue goes on.
- A queued prompt that cannot start (its model is gone, a plugin refuses it) keeps its place with the
  reason and holds the ones behind it until it is edited, sent or discarded.
- The hold after Stop is kept in memory, so after a restart the queue waits until a turn ends.

## Plan files

The plan agent reads and changes nothing, with one exception: it may write and edit Markdown files
in its workspace's plans folder, `<data>/plans/<workspace id>/`, which the environment block of
every agent's system prompt names. Any other change it attempts is refused with a message naming
that folder. Its prompt says to discuss the plan in the conversation and write a file only when the
user asks for one.

Every agent reads plan files without asking, so after switching to build the plan is there to read.
After a summary, the request lists the workspace's plan files (at most ten, by name, so editing
one never changes the cached request) beside the summary.

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

`provider_usage` (`src-tauri/src/usage_limits.rs`) takes the credential of each of the provider's
accounts, in the user's order, from Drift's own engine (`Engine::account_credentials`, the keyring),
calls the provider's usage endpoint for all of them at once, and returns one entry per account:
its key, label, and usage (null when that request failed) with normalized windows (kind,
optional label, percent used, and reset time in epoch milliseconds). Tokens never reach the
webview or a remote device. An expired sign-in is renewed through the engine's own refresh,
behind the same per-account lock a turn uses, so a rotating refresh token is never spent twice;
when renewal fails that account says the sign-in has expired. An account whose request failed
keeps the usage it showed before. The popover shows the account a turn would use: the first
with no window at its limit, else the first.
The frontend asks at most once a minute per provider, when the popover opens or a
session goes idle. **Settings > Usage limits** lists every linked provider that reports
limits, one block per account under the provider when it has several (its label, else
"Account N"), and its Refresh button bypasses the one-minute cache.

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
