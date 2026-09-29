# Engine rewrite

Drift is replacing the vendored opencode engine with its own engine written in Rust.
This document is the plan of record for branch `next/1.4.0-engine`. Keep it current:
when a decision changes, change it here first. Milestone status lives in `CHECKLIST.md`.

## Why

- The "never edit upstream" rule was already fiction. `engine/overlays/` held 28 patch
  files (about 500 KB of diffs) applied at build time. That is a fork maintained in the
  most fragile format available.
- Drift needs a small number of providers done extremely well, not thirty done adequately
  through a flattening SDK layer.
- The Drift app and the Drift engine share one process, one database and one author.
  A sidecar speaking a foreign API adds latency, a second storage engine and a second
  set of semantics for no benefit.
- Drift is its own product. The config files, env vars, data paths and API should say so.

## Decisions

| Area | Decision |
|---|---|
| Approach | Clean-room rewrite in Rust, test driven. Upstream opencode is read as reference only (gitignored clone under `examples/opencode`). |
| Topology | Engine is a library crate linked into the Tauri process. The frontend always talks HTTP + WebSocket on loopback so local and remote share one client. No sidecars. |
| API | New. HTTP for request/response, one WebSocket for events and replies with a resume cursor. OpenAPI generated with `utoipa`; the TypeScript types are generated from it and a thin typed request wrapper (`src/engine/native/client.ts`) is maintained by hand over them. |
| Providers | Native wire adapters: Anthropic Messages, OpenAI Responses and Chat Completions, Gemini, OpenAI-compatible generic. Presets over the generic adapter: OpenRouter, xAI, Z.ai, LM Studio, Ollama. Bedrock (hand-rolled SigV4, env and profile credentials) and Vertex (service account JSON and ADC file) reuse the Anthropic and Gemini adapters. |
| Catalog | models.dev JSON fetched and cached, filtered to supported providers, with a bundled snapshot fallback. Each entry carries a tool profile (`edit` or `apply_patch`). |
| Auth | API keys. Anthropic subscription OAuth (PKCE; the `@ex-machina/opencode-anthropic-auth` tarball is the spec). OpenAI Codex OAuth (upstream `plugin/openai/codex.ts` is the spec). Credentials stored with the `keyring` crate; encrypted file fallback on headless Linux. |
| Plugins | No JavaScript host. An internal `Hook` trait with serde-able input and output structs at the upstream hook points. Compiled Rust plugins through a Drift SDK come later and are not designed for now. |
| Tools | `read`, `edit`, `write`, `apply_patch`, `bash`, `glob`, `grep`, `webfetch`, `todowrite`, `skill`, `question`, `task`, `spawn_thread`, `read_thread`. |
| Dropped | `websearch`, `lsp`, `execute`, `plan`, share, ACP, TUI, CLI, Jev tool routing, Copilot, Azure, Cohere, Perplexity, GitLab, Venice, Poe, Alibaba, Gateway. |
| Edit | Exact match only, with line ending normalisation on both sides. On a miss, return the closest region so the model can re-read cheaply. `apply_patch` replaces `edit` and `write` for models whose catalog profile says so. |
| Post-edit | Formatter hooks only: built-in table, `drift.json` can add or disable, failures logged and never surfaced to the model. No language servers. |
| Snapshot and revert | Kept. Shell out to `git` with a shadow git dir per worktree. Snapshot before every writing tool. Revert restores a snapshot; diffs are computed between snapshots. |
| MCP | Native `rmcp` (stdio, streamable HTTP, OAuth). Approval, reconnect and reload designed in rather than patched on. |
| Storage | One `drift.db`, one writer, WAL, strict tables. Engine tables live beside the existing shell tables. |
| Config | `drift.json` at the project root, `.drift/{agents,commands,skills}/`, `~/.config/drift/`. Instructions from `AGENTS.md` and `CLAUDE.md`. Skills from `.drift/skills`, `.agents/skills` and `.claude/skills` at project and home. No runtime `opencode.json` fallback. |
| Identity | `DRIFT_*` env vars, `~/.local/share/drift` data dir. A one-time migrator runs on first launch. MIT attribution for opencode stays in `licenses/`. |
| Permissions | Upstream semantics (allow, deny, ask; path globs; session-scoped always; agent overrides) reimplemented once, with a single protocol. |
| Session tree | One tree: `parent_id` plus `visibility: hidden | sibling`. `task` creates a hidden child, `spawn_thread` a sibling. Subagent results stream into the parent as structured parts. |
| Platforms | Windows first. CI builds Windows, macOS and Linux. OS specifics live in one `platform` module. |

## Layout

```
Cargo.toml                 workspace
crates/drift-engine/       library
  src/
    api/                   axum router, ws hub, openapi
    session/               tree, turn loop, compaction, snapshot, revert
    llm/                   Provider trait, adapters, catalog, auth
    tool/                  Tool trait, registry, profiles, one module per tool
    edit/                  exact matcher, apply_patch parser, formatter runner
    mcp/                   rmcp client, approval, reconnect
    config/                drift.json, agents, commands, skills, instructions
    permission/ question/ hook/ store/ platform/
crates/drift-engined/      headless binary: parse args, run the engine
crates/drift-migrate/      imports opencode.db, opencode.json and auth.json once
src-tauri/                 depends on drift-engine; remote.rs stops proxying
tests/conformance/         bun test, black-box over HTTP and WS with recorded providers
```

One crate until compile times force a split. On this branch `engine/upstream`,
`engine/overlays`, `engine/opencode`, `scripts/build-engine.ts` and
`scripts/build-extensions.ts` are deleted once M1 passes.

## API

```
GET    /health                              version, uptime
GET    /openapi.json
GET    /workspaces          POST /workspaces
GET    /workspaces/{id}/config              merged drift.json, agents, commands, skills
GET    /providers           GET  /models     catalog with auth state
POST   /providers/{id}/auth/{method}        key, oauth start, oauth callback
DELETE /providers/{id}/auth
GET    /sessions?workspace=&cursor=&archived=
POST   /sessions                            {workspace, parent?, visibility?, title?}
GET    /sessions/{id}       PATCH           DELETE archives
GET    /sessions/{id}/messages?before=&limit=
POST   /sessions/{id}/turns                 {parts, model, agent, variant} -> 202 {turn_id}
POST   /sessions/{id}/abort
POST   /sessions/{id}/compact
POST   /sessions/{id}/fork                  {at_message?, bounded?}
POST   /sessions/{id}/move                  {workspace}
POST   /sessions/{id}/revert                {snapshot}
GET    /sessions/{id}/diff
GET    /sessions/{id}/todos
GET    /mcp                 POST /mcp/{id}/connect | disconnect | approve | auth
GET    /find/files?q=
WS     /events?cursor=
```

Server to client over the socket: `session.*`, `message.*`, `part.delta`,
`permission.asked`, `question.asked`, `todo.updated`, `mcp.*`. Client to server:
`permission.reply`, `question.reply`. Every event carries a monotonic `seq`. The first
frame is `hello` with the engine's random `instance` id and current `seq`. Reconnecting
with `cursor` replays from a ring buffer; a cursor that has aged out, or that is past the
head because it came from another process, returns `resync` and the client hydrates. A
client that sees a different `instance` in `hello` hydrates as well. Events that arrive
while a hydrate is in flight are held and applied after it, skipping any the snapshot
already covered.

## Data model

Added to `drift.db`: `workspace`, `session` (id, workspace_id, parent_id, visibility,
title, archived_at, model, agent), `message` (id, session_id, role, seq, created_at,
finished_at, cost, tokens), `part` (id, message_id, seq, kind, json), `todo`,
`snapshot` (session_id, message_id, git_ref), `permission_rule`, `mcp_server`,
`mcp_token`. Attachments reuse the shell's content-addressed `blob` table.

## Milestones

Exit criteria are the guard against scope creep. A milestone is done when every line
under it is true, not before.

### M0: skeleton

- Cargo workspace; `drift-engine` compiles and links into `src-tauri`.
- axum on loopback inside the Tauri process, `/health`, `/openapi.json`.
- WS hub with `seq`, ring buffer, `cursor` replay, `resync`.
- SQLite migrations for the tables above.
- Generated TS client wired into `src/engine/`.
- Perf baselines recorded against the current engine: cold start to first event,
  prompt-to-first-token engine overhead, system prompt plus tool schema tokens per turn.

### M1: vertical slice

- Anthropic adapter: API key and subscription OAuth, streaming, tool calls, thinking.
- Turn loop, `read`, `edit`, `write`, `bash`, `glob`, `grep`.
- Exact-match edit with closest-region miss reporting.
- Permissions end to end, snapshot before writes, persistence.
- Drift UI works against the new API for real tasks.
- Conformance tests with recorded Anthropic responses, including partial-chunk cases.

### M2: breadth

- OpenAI: Responses API, Codex OAuth, `apply_patch` profile. Gemini. OpenAI-compatible
  generic with presets.
- MCP through rmcp: stdio and HTTP, approval, reconnect, reload.
- `todowrite`, `skill`, `question`, `webfetch`. Async questions.
- Formatter hooks.
- Config loading: `drift.json`, agents, commands, skills, instruction files.

### M3: tree and lifecycle

- `task` subagents, `spawn_thread`, `read_thread`.
- Fork (bounded and active), move with busy guard.
- Compaction with recovery, retry with model switch.
- Revert and diff, shell timeout, per-session runtime config snapshots.
- Bedrock, Vertex, xAI and Z.ai presets.

### M4: cutover

- `drift-migrate`: sessions, messages, parts, todos, credentials to keyring,
  `opencode.json` to `drift.json` with a report of unmapped keys.
- Rename env vars and paths. Delete `engine/*`, `@opencode-ai/sdk`, overlays, build scripts.
- Remote gateway collapses into the engine router; device auth and TLS stay in `src-tauri`.
- Docs rewritten. Perf numbers against the M0 baseline published in release notes.

### M5: hook seam

- `Hook` trait finalised with serde types.
- Prompt overrides and MCP approval implemented as internal hooks to prove the seam.

## Working on it

- `cargo test -p drift-engine` for the engine, `cargo test --workspace` for everything,
  `cargo clippy --workspace --all-targets -- -D warnings` before any commit.
- `bun run gen:engine` regenerates `src/engine/native/types.ts` from the engine's OpenAPI
  (`drift-engined --openapi`). `tests/engine-client.test.ts` fails when it is stale.
- `bun run dev` starts the legacy sidecar (port 4196), `drift-engined`, and Vite; the
  browser reaches the native engine through `VITE_NATIVE_ENGINE_URL` and
  `VITE_NATIVE_ENGINE_TOKEN`.
- `bun run dev:shell` (with `bun run dev` already running) launches the desktop build as
  "Drift Dev" under the identifier `dev.drift.app.dev` (`src-tauri/dev.conf.json`). The
  identifier gives it its own single-instance mutex and its own data directory, so it runs
  beside an installed Drift and never touches that install's database.
- `bun run bench:engine [opencode|native] [runs]` measures the baselines below against a
  stub provider that answers instantly, so only engine time is counted.
- Every request carries `Authorization: Bearer <token>`; the socket takes `?token=` because
  browsers cannot set headers on a WebSocket. The shell hands the UI the token through the
  `native_engine_status` command.
- `drift-engined --file-credentials` keeps secrets in `credentials.json` under the data dir
  instead of the OS keychain; tests and CI use it so they never touch a real keychain.
  `DRIFT_ANTHROPIC_BASE_URL` points the Anthropic adapter at a fake for recorded runs.
- Claude subscription sign-in is the PKCE flow Claude Code uses (`llm/anthropic/oauth.rs`).
  Requests made with a subscription token must look like Claude Code's:
  `llm/anthropic/claude_code.rs` adds the identity and billing system blocks, prefixes tool
  names with `mcp_` and the adapter strips the prefix from what comes back. Subscription
  turns cost nothing, so their `cost` is recorded as zero.
- Tool calls run in the order the model issued them, one at a time. `edit` and `write`
  refuse files the session has not `read`; the first mutating call in a message takes a
  snapshot and records its tree id in the part's metadata.
- Ids are `prefix_<16 hex stamp><8 hex random>`; the stamp is milliseconds shifted left
  twelve bits plus a per-process counter, so rows made in the same millisecond still sort
  by creation.
- The engine owns the one connection to `drift.db` and the migration ledger. The shell's
  `Store` borrows it (`drift_engine::store::Store::lock`) for its own tables until they fold
  into the engine at M4. `workspace` is already the engine's table.

## Failure-path contracts

Settled after the first external review of M1; each has a regression test.

- A provider stream that ends without a stop reason is an error, not a completed message.
  Its tool calls stay `pending` and never run; the turn retries like any transport fault.
- Prompt admission is one transaction (`Store::admit_prompt`). If it fails, the session's
  busy reservation is released and nothing half-written remains. `Prompt.submissionId`
  is optional; resubmitting with the same id returns the original receipt, and reusing an
  id for a different session is rejected. The UI sends a fresh id with every prompt.
- Only `done` and `aborted` assistant messages are replayed to the model. `error` and
  `streaming` rows stay in the transcript as audit history and never enter a request.
- Token refresh is single-flight per provider: the first turn to notice an expired token
  refreshes it, later turns wait and reuse the stored result.
- The socket client never advances its cursor on a failed hydrate; it retries and keeps
  holding events. A `resync` that lands mid-hydrate folds into the same run.
- Session listings page until a short page; a snapshot is only authoritative when complete.
  Each listed session carries `running`, and the client sets status from it on hydrate.

Open, deliberately: one credential slot per provider (an API key and a subscription sign-in
replace each other; account profiles are M3 work), and the shell's `session_meta` archive
table still exists beside the engine's `archived_at` until M4 folds shell tables in.

## Baselines

Medians of five runs on the development machine, recorded at M0. The opencode numbers are
the target to beat; the native engine only has a cold start until M1 gives it a turn loop.

| Measure | opencode 1.18.33 | native (M0) |
|---|---|---|
| Cold start, process spawn to first event frame | 1012 ms | 26 ms |
| Prompt accepted to provider request sent | 1066 ms | |
| Provider response to text event delivered | 49 ms | |
| System prompt per turn | 12,089 chars | |
| Tool schemas per turn | 25,369 chars | |
| Approximate tokens per turn (chars / 4) | 9,365 | |

## Testing

- Rust unit tests per module. Edit matcher, `apply_patch` parser, SigV4, PKCE, catalog
  filter, permission rules and WS replay each get their own suite.
- `tests/conformance/`: bun test suites driving the HTTP and WS API against
  `drift-engined` through a recording provider proxy (own crate). Fixtures are committed.
  Upstream's test files are mined for scenarios, never copied as code.
- Perf: the three M0 numbers run in CI and fail the build on regression past a threshold.

## Effect on the app

- `src/engine/` talks only to the native engine. `native/client.ts` wraps the generated
  types, `native/events.ts` runs the socket, `actions.ts` implements every action the UI
  calls. Actions the engine cannot serve yet (fork, spawn, move, share, compaction,
  questions, revert, MCP, commands) raise a "not available yet" notice and return the
  neutral value their callers expect; each comes back native in the milestone that owns it.
- `native/adapt.ts` maps native sessions, messages, parts, permissions, providers and events
  onto the legacy store shapes (`@opencode-ai/sdk` types) that `store.ts`, `events.ts` and
  the components were written against. That keeps the whole UI working on the new engine
  without touching a component. At M4 the store adopts the generated types, the adapter
  goes, and `@opencode-ai/sdk` leaves `package.json`.
- Reasoning effort names from the composer (`low`, `medium`, `high`, `max`) become thinking
  budgets of 4k, 10k, 20k and 32k tokens.
- Workspace ids are shared: the shell's `workspace` table is the engine's, so the UI's
  workspace list resolves session workspace ids to directories without a second lookup.
- `src-tauri/`: `engine.rs`, `engine_db.rs`, `tool_routing.rs` and `usage_limits.rs` go.
  `remote.rs` loses its proxy half. `mcp.rs` shrinks as approval state moves into the
  engine.

## Risks

1. Anthropic subscription OAuth is a reverse-engineered flow and can break without notice.
   It sits behind the `AuthMethod` trait so a break is one file.
2. Streaming tool-call parsing differs per provider (Anthropic input JSON deltas, OpenAI
   Responses items, Gemini function parts). Recorded fixtures must cover partial chunks.
3. Branch divergence. `main` keeps shipping on opencode. `src/ui` changes merge cleanly;
   `src/engine` and overlay changes do not. No new overlay work lands on `main` once this
   branch is open.
4. Scope creep in M2 and M3. The exit criteria above are the guard.
