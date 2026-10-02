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
| Auth | API keys. Anthropic subscription OAuth (PKCE; the `@ex-machina/opencode-anthropic-auth` tarball is the spec). OpenAI Codex OAuth (upstream `plugin/openai/codex.ts` is the spec; it signs in with Codex's own OAuth client and sends `originator: opencode`, the value OpenAI accepts from that integration, so Drift keeps it rather than risk an unrecognised originator being refused). Credentials stored with the `keyring` crate; encrypted file fallback on headless Linux. |
| Plugins | No JavaScript host. An internal `Hook` trait with serde-able input and output structs at the upstream hook points. Compiled Rust plugins through a Drift SDK come later and are not designed for now. |
| Tools | `read`, `edit`, `write`, `apply_patch`, `bash`, `glob`, `grep`, `webfetch`, `todowrite`, `skill`, `question`, `task`, `read_thread`. M3 adds parent-scoped `task_output` and `task_stop` for background workers. Branch creation is never a model tool. |
| Dropped | `websearch`, the model-facing `lsp` tool (LSP diagnostics after edits come at M4), `execute`, `plan`, share, ACP, TUI, CLI, Jev tool routing, Copilot, Azure, Cohere, Perplexity, GitLab, Venice, Poe, Alibaba, Gateway. |
| Edit | Exact match only, with line ending normalisation on both sides. On a miss, return the closest region so the model can re-read cheaply; the tool text and the miss both say read's `N: ` prefix is not in the file, and a miss caused by copied prefixes says exactly that (still no fuzzy apply). `apply_patch`, offered only to the GPT and Codex models whose catalog profile asks for it, finds hunks as Codex's own `seek_sequence` does, because those models write patches that rely on it: exactly, then ignoring trailing whitespace, then surrounding whitespace, then with typographic dashes, quotes and spaces read as ASCII; the first pass that matches wins, and a miss shows the closest region as `edit` does. `apply_patch` replaces `edit` and `write` for models whose catalog profile says so. It follows the same rules: every existing file it adds over, updates, deletes or moves onto must have been read this session (so a secret needs its own read approval before it can reach a diff); every source and move destination is a separate edit ask (`Tool::asks`), any refusal refusing the call; and the whole patch is read, checked and worked out before any file changes. Only a missing file counts as absent; any other read error (denied, locked, a directory) stops preparation, and so does an update to a file that is not UTF-8 (a Windows-1252 page, say), which `edit` refuses too: decoding it loosely and writing it back would replace every such byte in the whole file. Every whole-file write the engine makes (`edit`, `write`, `apply_patch`, and undo and redo putting a file back) goes to a sibling file swapped into place (`tool::stage::replace`), so a failed write never truncates its target. Each replacement is one row in `staged_replacement` (migration 14): the destination, the staged sibling (`.<name>.drift-<8 hex>.tmp`) and the backup the swap may leave (same name, `.bak`), written in one statement before either file exists. After the swap, and at startup before any tool can run (`recover_leftovers`), the pair is settled: if the destination is missing and the backup exists, the backup is moved back first; only then are the siblings removed. The moment a swap succeeds the row is marked `swapped` (migration 15), before the backup is removed: from then on the backup is old content, so a backup that could not be removed yet (a scanner holding it) is only ever deleted later, never restored, even if the file has been deleted on purpose meanwhile. Until then it sits beside the file, so it can show in `git status`. If that move or a removal fails, both files and the row stay, and the next start tries again; rows are forgotten together in one short transaction only after their files are settled, and the store lock is never held across file I/O. A row whose paths are not exactly what the engine would name for its destination is dropped without touching any file. `write` treats only a missing file as new: a file it cannot read or decode still exists, so it must have been read first, and any other read error stops the write. On Windows an existing file is swapped with `ReplaceFileW` and no ignore flags, so its ACL and attributes carry over or the write fails; if the swap moved the original aside and could not put the new file in, it is moved back, and if even that fails the error names where the original is. A file another program holds open without delete sharing cannot be swapped: the write fails with that reason and the file is left as it was. On Unix the mode carries over; owner, group, extended attributes and POSIX ACLs are the new file's. On both, a file with other hard links is not written through them: the patched path gets a new file and the other links keep the old content. On a failure every step through the failing one is put back (a step already in its before state is left alone) and the error names any file that could not be. |
| Post-edit | Formatter hooks only: built-in table, `drift.json` can add or disable, failures logged and never surfaced to the model. A formatter that changed the file is named in the result ("the file no longer matches what you wrote; read it again"), so the next edit is not built on stale text. Then opt-in checks (`drift.json` `checks`, any linter or type checker) run once per step, their problems added to the step's last writing call; they stand in for LSP diagnostics until those land at M4. |
| Snapshot and revert | Kept. Shell out to `git` with a shadow git dir per worktree. Snapshot before every writing tool. Revert restores a snapshot; diffs are computed between snapshots. |
| MCP | Native `rmcp` (stdio, streamable HTTP, deprecated HTTP+SSE, OAuth), 2026-07-28 stateless servers found by probing with the handshake as fallback. Reconnect and reload designed in rather than patched on; no approval step. |
| Storage | One `drift.db`, one writer, WAL, strict tables. Engine tables live beside the existing shell tables. |
| Config | `drift.json` at the project root, `.drift/{agents,commands,skills}/`, `~/.config/drift/`. Instructions from `AGENTS.md` and `CLAUDE.md`: global (`~/.config/drift/AGENTS.md`), every directory up to the repository root, and subdirectories as their files are read. Skills from `.drift/skills`, `.agents/skills` and `.claude/skills` at project and home. No runtime `opencode.json` fallback. |
| Identity | `DRIFT_*` env vars, `~/.local/share/drift` data dir. A one-time migrator runs on first launch. MIT attribution for opencode stays in `licenses/`. |
| Permissions | Upstream semantics (allow, deny, ask; path globs; session-scoped always; agent overrides) reimplemented once, with a single protocol. |
| Session tree | Shared session storage, distinct ownership: `task` creates a hidden worker in foreground or background; the user spawns an independent sibling conversation with `/spawn <instruction>`. A conversation's parent link is provenance, not worker cancellation ownership. See "Subagents and branches" and "Background-worker implementation". |
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
    edit/                  exact matcher, apply_patch parser, formatter and check runners
    mcp/                   rmcp client, reconnect
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
POST   /sessions/{id}/turns                 {parts, model, agent, variant} -> 202 {session, message}
POST   /sessions/{id}/abort                 -> {aborted}
GET    /sessions/{id}/tasks                 workers the session launched, finished ones included
GET    /tasks/{id}                          one worker's state and result
POST   /tasks/{id}/abort                    stop one worker and nothing else
POST   /sessions/{id}/compact
POST   /sessions/{id}/fork                  {atMessage?}
POST   /sessions/{id}/move                  {workspaceId}
POST   /sessions/{id}/revert                {messageId}
POST   /sessions/{id}/unrevert
GET    /sessions/{id}/todos
GET    /mcp                 POST /mcp/{id}/connect | disconnect | auth
GET    /find/files?q=
WS     /events?cursor=
```

Server to client over the socket: `session.*`, `message.*`, `part.delta`,
`permission.asked`, `question.asked`, `todo.updated`, `mcp.*`. Client to server:
`permission.reply`, `question.reply`. M3 adds typed worker lifecycle/progress events
and generated completion inputs, separate from conversation creation.
Every event carries a monotonic `seq`. The first
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

For M3 background work, keep child transcripts in the same store and add
`subagent_job` plus parent-notification records. Persist job/run identity, parent
session/cancellation generation, originating admitted call, mode and selection
reason, chosen agent/model/config/tool revision, state, usage and result reference.
Terminal result and notification commit together; parent attachment is idempotent
by job/run identity. Do not store secret credentials in job JSON.

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
- MCP through rmcp: stdio and HTTP; reconnect/reload are M3 lifecycle work.
- `todowrite`, `skill`, blocking `question`, `webfetch`. Async questions are M3 work.
- Formatter hooks.
- Config loading: `drift.json`, agents, commands, skills, instruction files.

### M3: tree and lifecycle

- `task` foreground and background subagents; durable launch/completion delivery,
  bounded supervision, attributed approvals and restart interruption handling.
- User spawned threads (`/spawn <instruction>`); `read_thread`. Background workers remain
  tasks on the parent, not independent sidebar conversations.
- Async questions and MCP reconnect/reload deferred from M2.
- Fork (bounded and active), move with busy guard.
- Compaction with recovery, retry with model switch.
- Revert and diff, shell timeout, per-session runtime config snapshots.
- Bedrock, Vertex, xAI and Z.ai presets.

#### Subagents and branches

Two different things share the `session` table; `visibility` says which, never `parent_id` alone.
Product rationale: `docs/research/m3-conversations-and-subagents.md`.

| | Subagent (`hidden`) | Branch (`sibling` with `parent_id`) |
| --- | --- | --- |
| Purpose | Help finish the parent's goal | Pursue a separate goal |
| Created by | The model, through `task` | The user, through `/spawn` only |
| Context | A self-contained delegation prompt | A copy of the source's finished messages plus the user's instruction, with the source cutoff recorded |
| Output | Foreground returns a result; background returns launch receipt then a later completion input | Its own transcript |
| Stop | Explicit parent Stop cancels owned work; a parent turn naturally ending does not cancel background jobs | Independent; stopping the source does not stop it |
| Permissions | Inherits the parent's auto-accept | Its own |
| Sidebar | Under the parent while running, waiting on the user, or open | Normal top-level row, header links back to the source |
| History | Opened from the task card in the parent transcript | A conversation like any other |

- The current foreground `task` creates the subagent, titled `<description> (@<agent> subagent)`,
  and waits for it. The result follows how the child's turn ended, which the turn loop records
  (`TurnEnd`): a stop wins however it landed (mid-request, mid-tool or mid-compaction), otherwise the
  last attempt decides, skipping only *finished* compaction summaries. A reply (clipped at 20k
  chars) is the result; a failed or stopped turn fails the call, never falling back to an earlier
  reply or a summary. A reply that stopped at the output limit is `incomplete`: its partial text is
  kept in the result, marked as not a complete answer, and the call and task fail. Either way the
  call keeps `metadata.sessionId` and `metadata.outcome` (`replied`, `incomplete`, `failed`,
  `stopped`) for drill-down.
  Listings include subagent records for inspection; the sidebar shows only
  active/awaiting-attention workers. Background mode is described below.
- A finished subagent can be continued: `task` with `task_id` (one of this conversation's, not
  still running) records a new task on the same hidden session (`Store::resume_task`), so the
  subagent keeps everything it saw and the prompt only needs the follow-up; its agent stays as it
  was. Every result ends with `(task_id: ...)` so the model can do so; the UI's card hides that
  line. `task_for_session` answers with the newest task of a session.
- Delegation is one level deep: subagents are never offered `task` or `read_thread`, the tools
  refuse to run from one, and a subagent cannot be branched from.
- The model cannot create branches; there is no `spawn_thread` tool. It may suggest one in prose.
  `read_thread` lets a conversation check on a branch taken from it when the user asks.
- Spawning is one call, no drafting request and no review. `POST /sessions/{id}/spawn {instruction}`
  copies the source's finished messages (the fork copy, so compaction markers and boundaries carry
  over) into a sibling linked by `parent_id`, records `branch_cutoff`, titles it with the
  instruction's first six words, and submits the instruction as typed, on the source's model and
  level. Each request puts a "this is a new thread spawned from the conversation above; work only
  on what follows" line before the thread's first own prompt; it is never stored. Copied messages
  keep their times, so they are the ones older than the thread (`branch::is_copied`); the UI folds
  them behind one "Started with the conversation from <source>" row that expands them, as a
  compaction summary does, and shows the instruction as a folded "New thread" row. The model
  reads the copied conversation itself and decides what matters.
  It starts on its own abort token and returns 201 with the session. An empty instruction (400
  `instruction`) or a subagent source (400 `subagent`) creates nothing.
- A spawned thread differs from a fork only in its link to the source and its first prompt; see below.

#### Fork and move

- `POST /sessions/{id}/fork {atMessage?}` copies the source's finished messages and their parts,
  with fresh ids, into a new top-level conversation titled `<title> (fork)`, in one transaction.
  Without `atMessage` it copies through the last stable message: if a turn is running, everything
  from its user message on is left out. With `atMessage` it stops at that message, which must be
  finished and outside a running turn. A fork has no parent link; it is a copy, not a worker or
  a branch. The copy keeps compaction markers, so the fork sees the same context as its source;
  `/fork active` and `/fork all` are one operation. Boundaries are remapped to the copied ids.
- `POST /sessions/{id}/move {workspaceId}` moves the session and its subagents (hidden
  descendants at any depth). Branches stay where they are. It returns 409 while any of them is
  running or still planning, because a turn keeps the workspace path it planned with. A turn
  claims its session *before* planning (credential refresh included), and the move checks and
  updates while holding claims off (`Turns::while_idle`), so no turn is admitted in between.
- Re-pointing a workspace at a new folder moves nothing: sessions reference the workspace id and
  the shell rewrites the shared `workspace` row. The UI refuses while any session there is running,
  for the same reason as move.

#### Per-action models

Every job the engine does can run on its own model, chosen under Settings > Agents.

| Agent | Kind | Runs | Default model |
| --- | --- | --- | --- |
| `build`, `plan`, workspace agents | primary | conversations (picked in the composer); can also take a `task` | the one the conversation was prompted with |
| `general` (default `task` type), `explore` (read-only search), workspace agents with `mode: subagent` | subagent | `task` subagents only; listed for the model under "# Subagents" in the system prompt when `task` is offered | the parent's |
| `title` | action | naming a new conversation | the cheapest priced model from the conversation's provider (the conversation's own model when it is free, as with local providers) |
| `compaction` | action | summarising a long conversation | the conversation's |

- Settings overrides are stored by the shell (`prompt_override`, key `agent:<name>`, value
  `{ model: "provider/model", prompt }`). The shell hands them to the engine with
  `Engine::set_agent_overrides` at startup and after every save or reset. An empty `model` means
  inherit and masks a pin from the agent's definition. `Engine::workspace_config` applies them over
  built-in and workspace definitions, so Settings wins. Overrides for unknown agents are ignored.
- A workspace `.drift/agents/<action>.md` customises that action; it never turns it into an agent
  that can hold a conversation. `task` refuses action agents as `subagent_type`.
- Actions are one request each (`session::oneshot`), text only; tools are offered only so a
  history with tool calls stays valid, and calls the model attempts anyway are ignored.
- Titles: the first message becomes the title at once; the title model's answer replaces it in the
  background, only while the title is still that placeholder, so a rename wins. Any failure keeps
  the placeholder.
- Signed reasoning is replayed only to the model that produced it; any other model, including an
  action's, gets the history without it.

#### Compaction

Nothing is deleted. A compaction appends two messages: a user boundary holding
`Part::Compaction { auto, tailFrom }` and an assistant message with `summary: true` holding the
summary. The UI draws them as its existing collapsible "Context compacted" divider.

- **Request view** (`session::compaction::view`): the latest *finished* summary as the opening user
  turn, then every message from `tailFrom` on, skipping older boundaries and summaries (the new
  summary covers them). A failed or aborted summary, or one without text, is ignored, so the
  previous view stands.
- **Publication**: the summary's text and its `done` state are one transaction
  (`Store::complete_summary`). If that write fails the summary is marked failed, no completion is
  published, and the error returns to the caller (counted against automatic compaction).
- **Tail**: whole turns from the end, at most 2 turns and about 15k estimated tokens (4 chars a
  token), starting at a user message so tool calls stay with their results, and never reaching the
  first message: there is always something to summarise. When even the last turn is over budget,
  everything is summarised.
- **Summary request**: the `compaction` agent's model and prompt (per-action models above), the
  previous summary and the history before the tail, one text-only request. If the provider says it
  is too long, the oldest fifth of its turns is dropped with a note, up to three times.
- **Triggers**
  - Automatic, before each request in a turn: when the last finished reply since the latest summary
    used at least `context - reply room` tokens (`Model::reply_room`: the output limit, else a
    quarter of the window, never more than half a known window nor 32k; a 4k local model therefore
    compacts at 3k, and a model listing an output limit as large as its window at half, not before
    every step). A request's `max_tokens` is held to the same half. When models.dev gives an input
    cap below the window (`limit.input`; gpt-5.4 takes 922k of its 1.05M), that cap less
    `min(reply room, 20k)` is the point instead (`Model::compaction_point`), so a long Codex session
    compacts before the provider refuses it. The UI's context meter uses the same
    sum (`contextStats` in `src/engine/store.ts`), so "until compaction" is where it happens. One
    attempt per step; a failure still lets the request go.
  - Overflow: a provider error recognised as too long (`llm::Error::is_context_overflow`, status
    400 or 413 plus each provider's wording), or a reply that stops because it filled the window
    (Anthropic `model_context_window_exceeded`, kept as an `error` message so it is never
    replayed), compacts and retries once per turn; a second overflow fails the turn.
  - Manual: `POST /sessions/{id}/compact` (`/compact`) runs as the session's job, 409 while a turn
    runs, cancelled by Stop.
- **Off switch**: `GET`/`PUT /settings { autoCompact }`, stored in the engine's `setting` table,
  default on, shown in Settings > General. Three automatic failures in a row also stop it for that
  session until one succeeds; manual compaction always runs.

#### Retries

- What retries is decided once, in `llm::Error::api`: statuses 408, 429, 500, 502, 503, 504 and
  529, and for an error that arrives inside a stream that began 200 OK, its type
  (`overloaded_error`, `rate_limit_error`, `api_error`, `server_error`, `rate_limit_exceeded`,
  `UNAVAILABLE`, `RESOURCE_EXHAUSTED`, ...). A gateway's numeric code inside a streamed error
  classifies as that status. Transport failures, including a stream cut short, retry too. A
  permanent fault never retries whatever its status: `insufficient_quota` (which OpenAI sends as
  429), `billing_hard_limit_reached`, `billing_not_active`, `access_terminated`.
- The wait is the provider's when it names one: `retry-after-ms`, else `retry-after` in seconds or
  as an HTTP date. A value too large to represent saturates rather than failing, so it reads as
  longer than any wait we accept. `x-should-retry` overrides the classification either way, except
  that it cannot make a permanent fault retryable. Otherwise the wait starts at 1s and doubles with
  20% jitter, capped at 60s after the jitter.
- A job (a turn, a compaction) runs as its own task; if it panics, the session is still released,
  its retry wait cleared and `idle` published.
- At most 8 retries per step (the count resets after a step succeeds). A provider asking for more
  than 10 minutes, as a spent quota does, is not waited on: the error stands.
- The SSE parser decodes UTF-8 across network reads: a character split between two reads is held
  until its last byte arrives (text and tool arguments alike), and only bytes that can never form
  a character become U+FFFD.
- Every provider, OAuth, catalog and MCP HTTP request goes through one shared client
  (`llm::http::client`): shared connection pool, 15 s connect timeout, TCP keepalive. A provider
  request whose response has not begun within 120 s fails as a transport error, and a stream with
  nothing at all (not even a ping or comment) for 300 s ends as a stalled stream; both retry like any
  transport failure. Reasoning models can think silently for minutes, hence the long idle limit;
  both limits are per route (`Timeouts::for_route`): local routes (`ollama`, `lmstudio`) start at
  600 s for both, since loading a model or reading a long prompt on a CPU can take minutes, and
  drift.json `timeouts: { "<provider>": { "headersSeconds": n, "idleSeconds": n } }` sets any
  route's limits, applied when the turn plans and again if a retry switches its model. Stop ends a turn while its request is still being sent
  or waiting for the response to begin, not only once it streams. A body read whole rather than
  streamed (an error response, an OAuth token exchange) goes through `http::bounded_body`: at most
  64 KB and at most the idle limit (capped at 10 s), so an error that trickles in or never ends
  cannot hold the turn; whatever arrived is what the error says.
- Each wait publishes `session.retry { attempt, message, nextAt }`, which the UI draws as the retry
  notice with a model picker; `session.status running` follows when the wait ends. Stop ends a wait
  at once.
- Steering and queueing are the engine's. `POST /sessions/{id}/turns` on a session whose turn is
  running admits the prompt at once (durable, ordered by id, idempotent by `submissionId`) and
  returns 202; the turn takes it at its next model request, after the calls in flight finish, so
  their results come first in that request. Before a turn ends it checks, under the same lock that
  admission holds, for a prompt newer than the last one it answered; if there is one it carries on
  with a fresh step budget, otherwise it stops taking prompts, so none can land unanswered. A Stop
  still ends the turn; the steered prompt stays in the transcript for the next one. A session held
  by a job that is not a turn (a compaction, an undo) makes the prompt wait up to 30 s and then
  start a turn of its own; past that it is 409 `busy`.
- A prompt may switch the model, agent or level mid-turn, as in opencode. It is admitted like any
  steered prompt, and admission writes its choice onto the session. Before every request the turn
  reads the session (`follow_session`) and, if the choice changed, rebuilds what it runs on: the
  model, provider and credential, the agent's tools, system prompt and step limit, and the level.
  The conversation carries on as the new choice from that request; the prompt cache is lost. A
  model is checked when the prompt is sent (unknown model, no credentials: the sender gets the
  error); one that fails later pauses the turn with the reason. The prompt's files are judged
  against the model the next request runs on. Anthropic quietly drops thinking for a request
  that turns it on mid tool loop, so a level change there takes full effect on the next turn. The
  newest prompt always decides; the composer's own unsent pick changes nothing until it is sent.
  (A durable queue that gave such prompts a turn of their own was removed; migration 19 drops its
  table.)
- Turn limits (`config::Limits`, drift.json `limits: { steps, repeats, polls }`, later files
  override field by field; an agent's front matter `steps:` replaces `steps` for its turns):
  - `steps` (default 200): model steps that ran tools in one turn. Reaching it pauses the turn.
  - `repeats` (default 3): steps in a row whose calls, inputs and results are all identical. A
    different result is progress, so a poll whose answer changes never counts.
  - `polls` (default 30): the same for repeated steps whose shell commands wait on purpose
    (`sleep`, `Start-Sleep`, `timeout`, `wait`, `watch`), so deliberate polling is not taken for a
    loop.
  - A pause is a reply-less message with status `paused` and the reason in `error`; it is never
    replayed to the model, the UI draws it as a quiet interruption line with the reason, a subagent
    that pauses reports it to its parent as a failure with that reason, and the next message
    carries on.
- `POST /sessions/{id}/retry { model }` moves a waiting turn onto another model: 409 when nothing is
  waiting, 400/401 when the model or its credential is unusable (checked before the turn is told).
  The turn retries immediately on the new model, rebuilds its tools and prompt for that model's
  profile, and the session keeps the model for later turns.

#### Shell time limit

- A shell call runs for the model's explicit `timeout` if it gives one (capped at 24 hours, the
  Settings ceiling), otherwise for the user's Settings value (Settings > Tool execution). "No
  timeout" means none. Until the shell reports the setting the engine uses two minutes.
- The shell pushes the value with `Engine::set_shell_timeout` at startup and on every change; it
  applies to calls that start afterwards.
- The limit is in the call's metadata (`shellTimeoutMs`) from the moment it starts running
  (`Tool::running_metadata`), so the UI's badge shows it. A command stopped by its limit returns its
  partial output with `timedOut: true` and fails the call (`Tool::failed`); its process tree is
  killed either way.

#### Runtime snapshots

- A turn runs on what it was admitted with, its `Plan`: the workspace config (agents, skills,
  commands, permission policy, limits, formatters, route timeouts) read once, the model and
  provider, and its offer. The offer holds the tool objects themselves, not names: each MCP tool
  holds its server's client, so a server disconnected, replaced or reloaded while the turn runs
  keeps serving that turn's calls, and its client closes when the last turn holding it ends
  (`RunningService` cancels on drop). Calls resolve tools only from the offer.
- Tools that read config at call time (`skill`, `task`'s agent lookup) read the turn's snapshot
  through `Context::config`, never the files as they are now. The snapshot holds each skill's
  SKILL.md body as read with the config, so `skill` returns the instructions the turn was offered
  even if the file is rewritten while it runs; files a skill points to are read when used.
- Changes made meanwhile (a `drift.json` edit, Settings agent overrides, an MCP server connected or
  dropped) reach the next turn; an idle session picks them up when its next turn is admitted. Steered
  prompts join the running turn and its snapshot. A queued background worker keeps the snapshot it
  was admitted with, MCP clients included, until it runs. A user switching a waiting retry to another
  model rebuilds only the offer for that model's profile.
- Engine actions outside turns (titles, compaction summaries) make their own snapshot when
  they start.

#### Cloud routes

- Bedrock (`amazon-bedrock`) and Vertex (`google-vertex`, `google-vertex-anthropic`) find their
  own credentials, as the AWS CLI and Google client libraries do, each time a request is sent; the
  engine records only that some exist (`Credential::Ambient`, never stored), so the provider shows
  as connected from the environment. A cloud route's variables are never taken for an API key.
- Bedrock: a key saved in Settings, or `AWS_BEARER_TOKEN_BEDROCK`, is a Bedrock API key sent as a
  bearer token. Otherwise `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` (and `AWS_SESSION_TOKEN`), else
  the `AWS_PROFILE` (or `default`) section of the shared credentials file, sign the request with
  SigV4 (`llm::aws`, ring's HMAC-SHA256; the path is encoded once on the wire and again for the
  signature, which model ids with `:` need). Region: `AWS_REGION`, `AWS_DEFAULT_REGION`, the
  profile's `region`, else us-east-1. Requests go to `invoke-with-response-stream` with the Anthropic
  Messages body (no `model`, `anthropic_version: bedrock-2023-05-31`, cache breakpoints kept); the
  reply is AWS event-stream framing (`llm::eventstream`, CRC32-checked), each `chunk` an Anthropic
  stream event in base64. Exception frames (`:exception-type`) and error frames (`:error-code`,
  `:error-message`) end the stream with their reason; a frame of a kind this route does not know is
  a broken stream, never skipped. Each fault takes the status it stands for (throttling 429,
  service unavailable and model not ready 503, internal server and model stream errors 500, model
  timeout 504, validation 400), unless the response or the model gave one: a model stream error's
  `originalStatusCode` and `originalMessage` are kept, as is an HTTP error's status, with its kind
  from `x-amzn-ErrorType` and its retry headers. Access denied and bad or expired keys are
  unauthenticated. Only Claude models (`anthropic.` ids and inference profiles) are
  offered. SSO, `credential_process` and instance metadata are not read.
- Vertex: `GOOGLE_APPLICATION_CREDENTIALS`, else gcloud's application-default file. A service
  account key signs an RS256 assertion (ring) for a cloud-platform token; user credentials refresh
  theirs. The token is cached per credentials path and a hash of the file's contents, until five
  minutes before it expires, so rewritten credentials at the same path mint a new one. One exchange
  runs at a time; requests arriving meanwhile wait for it and use its token. The exchange is bounded
  by the route's time limits and its body read is bounded; refused credentials (`invalid_grant`,
  `invalid_client`, 401, 403) are unauthenticated, while Google's own trouble (429, 5xx) is a
  retryable error that keeps its `retry-after`. Project:
  `GOOGLE_VERTEX_PROJECT`, `GOOGLE_CLOUD_PROJECT`, then the file's `project_id` or
  `quota_project_id`; location: `GOOGLE_VERTEX_LOCATION`, `GOOGLE_CLOUD_LOCATION`, else `global`
  (whose host has no region). Claude models go to Anthropic's publisher (`streamRawPredict`, body
  with `anthropic_version: vertex-2023-10-16`), Gemini models to Google's
  (`streamGenerateContent?alt=sse`); both reuse their adapter's stream reading. Workload identity
  and the metadata server are not used.
- Both are verified against local stand-ins (signed path and headers, event-stream and SSE replies,
  token exchange and caching), not live accounts. xAI and Z.ai are OpenAI-compatible presets over
  the generic adapter, covered by its tests.

#### Async questions

- `question` defaults to `async: true`: the call registers the request and returns its id at once,
  and the turn carries on with what does not depend on the answer. `async: false` waits in the call
  as before. A subagent's question always waits, since its turn would end before a late answer
  could reach anyone.
- An answer (`POST /questions/{id}/reply`, or `question.reply` on the socket) is written as its own
  user message holding a `clarification` part (header, question and answers per item; the model reads
  it as a `<question-answer>` block, the UI as its Answered row), with submission id `answer:<id>`.
  It goes through the same admission as any prompt: it joins a running turn at its next request, or
  starts one if the session is idle. If the session has been stopped since the question was asked
  (its durable Stop count moved), or another job holds it past the queue wait, the answer is only
  saved, starting nothing; the next turn reads it. Held worker results ride along like with any
  prompt.
- The card closes only after the answer is saved; a failed save (no model, no credentials) leaves it
  pending and answerable, and the route returns the error. Declining closes the card and says nothing.
- Decisions on one question take turns: answering and declining hold a per-request lock and check the
  card is still pending after taking it, so an answer and a dismissal racing never both go through.
  The loser sees the winner: a dismissal after a saved answer is 409, an answer after a dismissal 404.
- Whether an answer already landed is read from the store, not from memory: the submission id is
  checked inside the admission's own write, so two identical answers racing write one message and
  both succeed, and a different answer under the same id is 409 with nothing written. Once the card
  is gone (settled, or the engine restarted) a resent answer is compared with the saved
  `clarification` part: the same answer is accepted, any other answer or a dismissal is 409.
- A `question.reply` sent on the socket is answered on that socket with a `question.result` control
  frame (`requestId`, `ok`, and the same `error` body the HTTP route would return). It carries no
  `seq`; clients that reply over HTTP ignore it.
- Pending questions live in the engine process: a restart drops unanswered cards, never a saved
  answer. Deleting a session drops its async questions.
- A prompt sent through the API carries text and files only: `task_result`, `clarification`, tool and
  other engine parts are refused with 400, so a client cannot forge a worker result or an answer.

#### Undo and redo

- Every writing call records what it changed as `metadata.changes: [{path, before, after}]`, blobs
  in a shadow git dir under the data dir (`session::changes`). File tools name their paths
  (`Tool::touches`), so exactly those files are recorded before and after the call, after
  formatters run. A shell command can change anything, so the tree (the workspace's `.gitignore`
  applies) is compared just before and after it, unless the line only reads (`command::reads_only`:
  every command a known reader such as `git status`, `ls`, `rg` or `Get-Content`, nothing hidden,
  no redirection that writes), which is neither captured nor waited for like a write
  (`Tool::call_mutates`). That comparison shows what changed, not who changed
  it: the user, an editor or another session may have written during the command. Those changes are
  recorded with `observed: true`, and undo and redo never apply them; they are listed as
  `unattributed` and shown in their own notice. A path whose recorded history includes any observed
  change is left alone as a whole. A call whose files cannot be recorded does not run.
- A record that fails after the call (`record_call`) is never dropped. For a file tool the named
  files go back to their recorded before state, the call fails, and its result says so; any file
  that could not be put back is listed in `metadata.unrecorded` with the reason in
  `metadata.historyError`. A shell command's tree cannot be put back, so its result and
  `historyError` say undo cannot restore what it changed.
- The shadow repo stores exact bytes. Its `info/attributes` (which outranks every in-tree
  `.gitattributes`) unsets `text`, `eol`, `filter`, `ident` and `working-tree-encoding`, so a CRLF
  file under `* text=auto` or a file with a clean filter is stored as it is on disk and compares
  equal to what undo hashes with `--no-filters`. Repos made before the rule get it on their next
  capture, and their index is dropped then so no cached converted entry survives.
- Cost, measured with `session::snapshot::tests::capture_cost` (release build, Windows): a first
  capture of 5,000 1 KB files takes 2.1 s; an unchanged one about 90 ms, of which the size walk is
  13 ms. This repository (7,161 tracked files) takes about 190 ms unchanged, 70 ms of it the size
  walk. A shell call pays two captures. Git's stat cache already makes an unchanged capture cheap,
  and `core.untrackedCache` measured no better. The previous call's tree is never reused as the next
  call's before state: nothing short of a filesystem monitor establishes that no one edited in
  between, and a stale before state would put the wrong content into undo.
- Undo and redo stop a running turn first (as Stop does, workers included) and wait up to 15 s for
  it to end, then go ahead; only a job that does not stop in time makes them 409 `busy`.
- `POST /sessions/{id}/revert { messageId }` takes a prompt the user sent. It hides that prompt and
  everything after it (`session.revert.messageId`, the UI filters) and puts each file the hidden
  turns changed, subagents included, back to
  its state before the first of those changes. Only if the file still holds what the session last
  wrote: anything changed since is kept, never overwritten, and listed (`kept`, shown as a notice).
  No other file is read or rewritten. Calling revert again moves the point: back undoes the range
  in between, forward redoes it, with the same check.
- An undo or redo is all or nothing. Each file goes through the staged writer; if one cannot be
  written (on Windows, a program holding it open without delete sharing is enough), the files this
  call already changed are put back, newest first, to what they held before it, the conversation is
  not marked, and the error says so or names any file that could not be put back. The journal of
  applied files is kept until the undo point is saved: if that save fails, the files go back the
  same way, so files and history never disagree. A retry then finds every file as the session left
  it, so none is wrongly reported as kept.
- `POST /sessions/{id}/unrevert` redoes the hidden range the same way and clears the marker.
- Calls are replayed in the order their writes finished: each record carries `at`, a time-ordered
  stamp taken once the call's writes (and formatters) are done, so two workers that start in one
  order and write in the other are still undone as they happened. Records made before `at` use
  their message's id.
- Changes to one path merge only while they chain: each change's `before` must equal the previous
  one's `after`. A gap means someone else edited the file between two of the session's writes;
  that path is kept and reported in both directions, since undoing to the first `before` (or
  redoing to the last `after`) would erase their edit.
- The shadow repo is one per workspace, shared by every session and subagent in it, so creating it
  and every index operation (tree captures, prunes) hold a per-workspace lock.
- History belongs to the workspace, not its path: the repo is named for the workspace id
  (`Snapshots::bind`, taking over a repo kept under the old path-derived name once), and every
  recorded call stores its `owner` workspace id beside its changes. Undo and redo apply each change
  where its owner's directory is now, whichever workspace the session has moved to, so a session
  moved from A to B undoes A's files in A with A's history; a workspace pointed at a new directory
  keeps its history there. Records older than `owner` use the session's workspace; a change whose
  workspace no longer exists is kept and reported.
- Files over 10 MB are never copied into it: a file tool refuses to change one, or to write
  content that would make one (`edit`, `write` and `apply_patch` check the prepared bytes before
  anything changes), since the change could not be undone, and tree captures leave them out through the shadow repo's own exclude file
  (the workspace's `.gitignore` is untouched). Exclusion only stops untracked files, so each capture
  also removes oversized paths from the shadow index before adding: a file recorded while small that
  grows past the limit never enters the object store. Each capture carries the oversized paths it
  left out with their size and mtime, and a path oversized on either side of a comparison is
  reported in `metadata.unrecorded`, never as a deletion or creation, so undo cannot delete a large
  file or restore a stale small copy over it. An oversized file left untouched is not reported.
- Retention: `Engine::maintain` runs at startup and every six hours while the engine is up (it
  holds only a weak reference, so it never keeps an engine alive). Each pass, `prune_snapshots`
  pins every blob a stored call's `changes` refers to (archived sessions included), grouped by the
  owner recorded with it, under a private ref in that owner's repo and prunes the rest (the rows are
  read under the database lock with a SQL filter; their JSON is parsed after it is released), sparing objects younger than two hours so a capture in flight keeps its
  blobs; tree captures are transient and go too. The same pass deletes spooled shell output older
  than seven days.
- Nothing is deleted while undone. The next prompt commits the undo: the hidden messages and their
  submission records are deleted in the same transaction that records the prompt, each announced
  as `message.removed`, and the files stay as the undo left them.
- Both hold the session like a turn (409 while one runs). While undone, a fork copies only the
  visible part, a branch draft reads only the visible part, and compaction is refused (409
  `reverted`) because its summary would land among the messages the next prompt deletes.
- There is no session-wide diff endpoint: nothing consumes one. Per-call diffs travel in tool
  metadata.

#### Background-worker implementation

RE evidence, exact binary offsets, selection paths and the acceptance matrix are
in [Claude async workers](research/claude-async-workers.md). It extends the earlier
child-runner trace. The installed 2.1.85 binary has both sync and async Agent paths;
its fork-mode helper is compiled off. Skill `context: fork` selects another context
and is synchronous there. Neither the word workflow nor fork in chat selects a
runner. Current Claude documentation has additional version-specific defaults;
Drift uses the explicit contract below rather than copying hidden feature gates.

1. **Resolve execution mode once.** Extend `task` with optional `run_in_background`.
   Explicit true/false wins; omission uses the selected agent's configured execution
   default or foreground. Store mode and reason. One typed resolver serves all
   worker-producing entrypoints. Context inheritance and independent conversation
   branching do not force async. A disabled async feature rejects an explicit
   background request clearly. No prose/XML-like keyword detection or model-ID
   heuristics. Skill loading remains inline unless metadata explicitly requests a job.
2. **Admit then launch.** Foreground continues to await its compact result. Background
   commits a hidden job/child identity and returns a launch receipt without waiting
   for completion. Receipt means queued/running, never completed. Repeated delivery
   of the originating admitted call resolves to the same job.
3. **Supervise in the engine.** Use the same runner/adapters with a bounded Tokio
   worker queue/semaphore, step limits, usage and cancellation. Workers outlive the
   launching tool future and UI view. Main and several independent workers can
   make progress concurrently; no second model runtime or external engine process.
4. **Deliver results at safe boundaries.** Commit completed/failed/cancelled result
   and a unique pending notification together. Attach ready results once to durable
   parent context with engine-origin provenance. Keep the launch tool result as
   launched; never overwrite it or create a second result for the same call.
   Group ready completions. Active parents consume them between provider steps;
   idle follow-up is serialized under the delegation's continuation policy.
5. **Distinguish idle from Stop.** A normal parent turn ending does not stop async
   work. Explicit session Stop cancels owned workers even when the parent is idle.
   Individual worker Stop affects only that worker. Cancel generations suppress
   late continuation, and shell descendants follow existing process-tree cleanup.
   A user-created branch is not owned by this cancellation scope.
6. **Keep permissions attributed.** Worker requests appear under their owning parent
   with job/call identity. Only user approval unblocks the specific request. A main
   or sibling agent's message is not permission. Pin agent/model/tools/config at
   launch and keep no nested delegation. Ordinary workers start from a complete
   prompt; full-context worker forks can follow bounded/active fork implementation.
7. **Expose job state without polling.** Add the task query/abort routes above and
   lifecycle events through generated OpenAPI. Parent-scoped `task_output` returns
   current state/result without blocking by default; explicit wait is bounded.
   `task_stop` cancels one owned worker. Passive completion notification is normal;
   the model must not repeatedly sleep/poll or invent results before it arrives.
8. **Recover without rerunning effects.** Persist terminal results and undelivered
   notifications. On process restart mark nonterminal jobs interrupted; never
   automatically rerun provider calls or external mutations. Explicit resume, when
   added, uses a new job-run identity. Reconnect hydrates jobs/asks rather than
   starting execution. Old view generations cannot land stale progress/results.
9. **Retain history without sidebar clutter.** Completed workers leave active
   indicators. Their compact result and inspectable child transcript stay in the
   parent's task history, subject to retention. Parent compaction carries outstanding
   job IDs and delivery state, not every worker transcript.

What is built (`session::tasks`, `store::tasks`, `tool::task`):

- Every `task` call is a row in `task` (migration 9), unique per parent session and call id. It
  records owner, worker session, agent, `mode` and `reason`, `state` (`queued`, `running`,
  `replied`, `failed`, `stopped`, `interrupted`), the result text, whether the parent has it
  (`delivered`) and the owner's Stop count at launch (`generation`, migration 10).
- Identity: `Store::launch_task` creates the row and its hidden child session in one transaction,
  or returns the pair the call already made. A repeated call creates nothing and gets what it
  launched in its own mode: a background receipt, or the foreground result once that worker ends.
- `resolve_mode(explicit, agent default, enabled)`: `run_in_background` if given, else the
  agent's front matter `background: true|false`, else foreground. With the Settings switch off
  (`backgroundTasks`, on by default) an explicit request fails the call with the reason and an
  agent default falls back to foreground, recorded as `background turned off`.
- Admission (`Engine::admit_worker`): each worker gets its own cancellation token, registered
  before it is planned or queued; a background worker's descends from its owner's scope, a
  foreground one's from the launching call. The worker's turn is planned then, under that token:
  session, workspace, config (agent prompt, policy, limits, formatters), model, provider and the
  offered tools and system prompt are fixed in the `Plan`. A queued worker runs on that plan; when
  it starts (`submit_planned`) only the credential is looked up again, and a session that moved
  workspace fails it (`TurnError::Moved`) instead of quietly re-planning.
- Foreground: runs to its end within the call (`Task` stops itself, so a Stop waits for the worker
  to wind down and be recorded), and its result is the call's result.
- Background: the call returns a receipt at once (`outcome: launched`). The worker waits for one
  of four slots (`MAX_BACKGROUND`) or its token, whichever comes first, and checks its token again
  after getting a slot, before it is marked running. Its turn's abort token descends from the
  worker's, so Stop reaches it while queued, starting (before its turn claims the session),
  planning and running.
- Handing a result over is claimed and transactional. A claim (`Workers::claim`, in memory: the
  engine, or one call of the parent's) gives one path at a time the right to attach the result.
  `delivered` is set only in the same SQLite write that saves what carries it: the prompt
  (`Store::admit_delivering`) or the call's result (`Store::save_part_delivering`, via
  `settle_delivering`). A tool marks its output with `metadata.delivers`, honoured only if that
  call holds the claim. When a call's claim is released (at the end of every call, and when a turn
  job ends) a background result still owed goes out automatically, so a call whose result failed to
  save leaves it owed, not lost.
- Automatic delivery: a replied or failed background result is submitted to the parent as an
  engine-origin prompt holding a `task_result` part (`<task-result id= description= outcome=>` for
  the model; synthetic text in the UI) with submission id `task:<id>`. A running parent takes it at
  its next request (steering); an idle one starts a turn for it. Stopped and interrupted workers
  only ever steer into a running turn (`Admission::steer_only`). `task_output` claims the task while
  it waits, so a result finishing meanwhile comes back as its output and not also as a message; one
  already being delivered is reported as arriving, without its text.
- Stop fencing: every Stop takes the workers' owner lock, bumps the owner's durable Stop count
  (`stop_generation` table), cancels the owner's scope, the running turn, and for a worker's own
  transcript that worker. The last check before any prompt is written (`admit_fenced`) runs under
  the same lock, so a prompt lands wholly before a Stop or not at all. A delivery carries the
  owner's scope from the launch's generation as its parent token through every wait (a job holding
  the session, planning, credential refresh) and into that final check.
- Retry while running: a delivery that cannot be admitted stays owed and records why
  (`deliveryError`, migration 13, shown in the tasks dock; cleared when it is handed over or held).
  `Engine::retry_deliveries` tries owed results again, once each, when the parent's job ends
  (`spawn_job` cleanup, after the session is idle) and on repairs: an API key or sign-in saved, the
  session's model or agent changed (`PATCH /sessions/{id}`), Settings agent overrides pushed. A
  wait for a busy parent (`QUEUE_WAIT`) ending in `Busy` is retried by the first; a missing model,
  credential or workspace by the second. Nothing retries on a timer, so a permanent failure stays
  owed without a loop. Readiness and claims are coordinated: a trigger that finds the result
  claimed marks the claim, and the attempt holding it tries once more when it lets go, so an idle
  transition racing a failing attempt is never lost. Two automatic attempts never run at once.
- Held results: a result Stop keeps from waking its parent (launched before the owner's latest
  Stop, or a stopped or interrupted worker with no turn running to take it) is not marked
  delivered. It is marked `held` (migration 12) and stays owed: automatic delivery and restart
  recovery skip it, so it never wakes the parent itself. The next prompt admitted into the parent
  carries it ahead of its own parts, whether the user sent that prompt or it is another result's
  permitted delivery; every acknowledgment, the delivery's and each rider's, is in the same write,
  and a delivery that already landed writes nothing, its riders included.
  Each held result is claimed for that admission, so `task_output` and the prompt cannot both carry
  it. `delivered` therefore always means a saved prompt or call result holds it, except rows a
  restart interrupts, which are settled at startup as before.
- Stop: session Stop (`POST /sessions/{id}/abort`) reaches its workers even with no turn running
  and reports whether anything was stopped. `task_stop` and `POST /tasks/{id}/abort` cancel one
  worker's token; a Stop that arrives before the worker registers waits for it. Worker permission
  and question asks carry the worker's session id and inherit the owner's approvals.
- Restart: opening the store marks rows still `queued` or `running` `interrupted` (never rerun,
  never delivered as a prompt), before anything in the new process can launch a worker. Once the
  engine listens, `recover_tasks` handles owed results by mode. A background one is delivered as
  above, compared against the durable Stop count, so a Stop before the restart still suppresses
  it. A foreground one is never a prompt: if its launching call's result never landed (still
  pending or running, or failed to save), the result is written into that call and marked handed
  over in the same write; otherwise it is settled.
- `task.updated { task }` is published on launch, start, ending and delivery.
- `task_output { task_id, wait_seconds? }` answers for the calling conversation's own background
  tasks only, waiting at most 120 s. A foreground task is refused: its result belongs to the call
  that launched it, which holds the claim from the moment it returns until its result is saved, and
  if that save fails the result is recovered into that call, never taken by another. A worker that
  could not start, or was stopped, is still the launching call's own (failed) result, handed over
  in the write that saves it. `task_stop { task_id }` stops one of the conversation's tasks. Neither
  is offered to subagents.
- UI: the engine store keeps `tasks[parentSessionId]`, loaded with the transcript
  (`GET /sessions/{id}/tasks`) and folded from `task.updated`. A task only moves forward, so a
  snapshot that raced an event never replaces the newer record. The transcript's `task` row takes
  its state from the record (matched by `taskId` metadata or call id): a background receipt is a
  finished call but stays running until its worker ends, and reads failed when the worker failed,
  stopped or was interrupted. Above the composer, a Background tasks dock lists the conversation's
  background workers while any is queued, running or not yet delivered, each with its state, the
  running worker's current tool, Stop (`POST /tasks/{id}/abort`) and a link to its transcript.
  Foreground workers are not listed there; their row in the transcript already waits for them.

Initial async mode is selected at launch. Foreground-to-background promotion,
agent teams, arbitrary cross-agent messaging and automatic post-crash execution
resume are not required for the first implementation. `/spawn` stays a user-only,
one-call spawn with an independent session lifetime.

#### Async worker acceptance gates

- Controlled slow background worker returns a receipt and the main does useful
  work before the result arrives; foreground mode still waits. Two workers can
  finish out of order with correct attribution and bounded concurrency.
- Resolver fixtures cover explicit true/false, agent default, omission and disabled
  background mode. Skill-context inheritance and workflow-like prose cannot silently
  change the execution choice.
- Pending permissions/questions block only the right job; denial, individual Stop,
  session Stop and parent naturally idle have distinct outcomes.
- Completion races Stop, parent input and reconnect without reviving canceled work
  or attaching a result twice. Launch receipt is not a second completion tool result.
- Restart around terminal result/notification commit preserves delivery and marks
  unfinished jobs interrupted, with no automatic replay of uncertain effects.
- Completed workers disappear from active UI, remain inspectable in parent history,
  and never become permanent conversation rows. UI reload/parent compaction preserves
  outstanding-job identity. Branch Stop remains independent.

Use fake providers, controlled tool barriers and drain/receipt signals. Ordinary
tests require no paid inference. Existing foreground/branch checks stay checked;
these async criteria are new pending M3 work.

### M4: cutover

- `drift-migrate`: sessions, messages, parts, todos, credentials to keyring,
  `opencode.json` to `drift.json` with a report of unmapped keys.
- Rename env vars and paths. Delete `engine/*`, `@opencode-ai/sdk`, overlays, build scripts.
- Remote gateway collapses into the engine router; device auth and TLS stay in `src-tauri`.
- Docs rewritten. Perf numbers against the M0 baseline published in release notes.

### M5: hook seam

- `Hook` trait finalised with serde types.
- Prompt overrides implemented as an internal hook to prove the seam.

## Working on it

- `bun run gates` runs every gate: it builds `drift-engined`, then checks the generated client,
  clippy and `cargo test --workspace` beside typecheck and `bun run test` (files in parallel),
  stops at the first failure and prints only that. A link that fails because Windows antivirus
  held the fresh binary is retried once. About 40 s after an engine edit on a warm build.
- While iterating, `cargo test -p drift-engine --lib -- <filter>` (about 5 s to rebuild); dev
  builds keep line tables only, which cuts that rebuild by about a third.
- `bun run gen:engine` regenerates `src/engine/native/types.ts` from the engine's OpenAPI
  (`drift-engined --openapi`); `bun scripts/gen-engine-client.ts --check` fails when it is stale.
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
- An API-key Anthropic request with a thinking budget and tools sends `anthropic-beta:
  interleaved-thinking-2025-05-14`, so the model thinks again between tool calls, not only before
  the first; the subscription route already sends it, and adaptive thinking interleaves without it.
  Bedrock and Vertex do not send it yet.
- Anthropic prompt caching uses all four breakpoints: the last tool, the system prompt, and the
  last cacheable block of the two newest user messages. The newest writes the whole prefix; the
  one before it sits exactly where the previous step wrote, so a tool loop pays only for each
  step's new blocks even past the 20-block lookback. Thinking blocks and empty text never carry
  one. The marks are set in the adapter's shared body, so key, subscription (whose extra system
  blocks add none) and Anthropic-dialect gateway base URLs all cache alike.
- OpenAI caching is automatic, keyed by routing: every request of a conversation carries
  `prompt_cache_key` = the session id (API key and Codex routes alike, as Codex itself does), so
  its steps and turns stay on one cache. Title and compaction requests carry none.
- Chat Completions routes (xAI, Z.ai, OpenRouter, LM Studio, Ollama) stream text and reasoning as
  they come, but gather tool calls whole by `index`, since gateways interleave calls' deltas, and
  hand them on after the stream ends, in index order, one start each. A stream that ends (`[DONE]`)
  without ever giving a `finish_reason` is a failed reply: its calls are not run, as with any stream
  that ends without saying why.
  - A turn's `tool` messages go straight after the assistant message that called them; anything
    else in that user turn (the line introducing a returned image, a prompt steered in mid-turn)
    follows them as a `user` message, since these APIs refuse anything between calls and results.
  - Reasoning a model streamed as `reasoning_content` goes back to the same model, on the assistant
    messages of the tool loop under way (after the latest `user` message) and nowhere earlier:
    Kimi, GLM and DeepSeek thinking models expect it there, and ignore or refuse it from earlier
    turns. Only finished replies give it; Anthropic never receives unsigned thinking.
- Local servers report their own models (`llm::local`). Every 15 s the engine asks LM Studio and
  Ollama (`/v1/models`, plus LM Studio's `/api/v0/models` for kind, context and tool support) at
  their default address or the one the user set; what answers replaces that provider's listed
  models (embedding and tool-less models left out). Context is the window the server really runs
  with, since both cut longer prompts: LM Studio's loaded length for a loaded model and unknown (0)
  for one it would load with its own default; for Ollama, a loaded model's allocated
  `context_length` from `/api/ps`, else the model's own `num_ctx` from `/api/show` (asked once per
  installed build, keyed by its `/api/tags` digest on the engine, so a model re-created under its
  name is asked again; never more than its trained length), else unknown. The same answer's
  `capabilities` decide the rest: a model without `tools` is left out, and one with `vision`
  reads images. The window falls back to unknown,
  since Ollama's default depends on the server's memory (4K below 24 GB of VRAM, 32K to 48 GB,
  256K above, checked 2026-10 against docs.ollama.com) and Drift's environment says nothing about
  a server elsewhere. An unknown window becomes known within a poll of the model loading; until
  then the composer's picker says so, since a turn planned without it cannot compact. The
  provider counts as connected with no key while it answers, and `catalog.updated` tells the UI.
- The user's own `~/.config/drift/drift.json` may add or re-point providers:
  `providers: { "<id>": { name?, baseUrl?, apiKeyEnv?, models?: { "<model>": { name?, context,
  output, images } } } }`. A known id gets its endpoint replaced (a gateway, a remote LM Studio) and
  any listed models added; a new id is an OpenAI-compatible route, keyless unless `apiKeyEnv`
  names its key. A project's drift.json cannot do this: a committed file must never send your key
  elsewhere. models.dev's own `api` field is ignored for the native routes, so only the user
  re-points those; `DRIFT_<ID>_BASE_URL` still wins for recorded runs. A subscription sign-in is
  never sent to a re-pointed route (its token and Claude Code or Codex identity would go to the
  gateway): such a turn refuses with 400 `config`, and an API key for that provider works.
- OpenRouter is a catalog provider (`openrouter`, from models.dev at the first refresh; the
  bundled offline snapshot does not list it). Its Chat Completions route caches Claude only with
  explicit `cache_control`: a top-level field for automatic caching, which OpenRouter supports only
  for some upstreams, or per-block breakpoints (at most four), which it passes to every
  Anthropic-compatible upstream, Bedrock and Vertex included (OpenRouter prompt-caching guide,
  checked 2026-10-01). Drift uses the per-block form, placed as the Anthropic adapter places it:
  the system prompt (sent as a text block) and the last text of each of the last two user turns,
  three breakpoints. A user turn there is the run of `tool` and `user` messages between replies,
  so in a tool loop the breakpoint lands on the newest tool result (its content sent as one text
  part to carry it), not back on the prompt that started the loop. It applies only to `anthropic/...` (and `~anthropic/...` alias) models on that
  route (`Compat::caching_claude`); every other model and gateway is sent unchanged. Usage reads
  `prompt_tokens_details.cached_tokens` and `cache_write_tokens`. Verified with a recorded exchange
  against a local stand-in, not a live account.
- Tool calls keep the order the model issued them in, but not one at a time: a run of consecutive
  read-only calls executes together, a mutating call waits for everything before it, and reads
  after it wait for it (see the M1 guarantees). Foreground `task` workers run within their parent's
  call; background workers and other sessions run alongside. The shadow-index lock serialises
  snapshot captures per workspace only; it does not coordinate two sessions or workers editing the
  same source file, which is what the read-before-write check and undo's kept files are for.
  `edit`, `write` and `apply_patch` refuse existing files the session has not `read`; every
  mutating call records what it changed in its part's metadata.
- Ids are `prefix_<16 hex stamp><8 hex random>`; the stamp is milliseconds shifted left
  twelve bits plus a per-process counter, so rows made in the same millisecond still sort
  by creation.
- The engine owns the one connection to `drift.db` and the migration ledger. The shell's
  `Store` borrows it (`drift_engine::store::Store::lock`) for its own tables until they fold
  into the engine at M4. `workspace` is already the engine's table.
- Loading messages is two queries whatever their number: the page of messages, then every part in
  that id range in one join (`with_parts_in`), so the shared lock is held briefly even on long
  sessions. A step loads only what a request can show (`request_window`): after a compaction, from
  the latest finished summary's kept tail (`Store::view_start`: the boundary's `tailFrom`, else the
  boundary) on, never the history already summarised; before any, all of it. A summary with no
  text stands for nothing, so then the whole transcript is loaded and the view reaches past it. It
  loads once per step (twice when it compacts first), and a compaction summarises from the same
  window; the loop check, the subagent outcome, closing unrun calls and the end-of-turn steering
  check load only the message or id they need (`Store::last_reply`, `Store::with_parts`,
  `Store::newest_prompt`).
  Together these do what opencode's `zz-prompt-row-scan` and `zz-prompt-context-bounds` overlays did.

## Failure-path contracts

Settled after the first external review of M1; each has a regression test.

- Terminal contract for a streamed response: the engine requires a stop reason (Anthropic
  `message_delta.stop_reason`, OpenAI `response.completed`/`response.incomplete`, Gemini
  `finishReason`, Chat Completions `finish_reason` then `[DONE]`). `message_stop` alone is
  not completion. A stream that ends without one is an error, not a completed message: its
  tool calls never run, and the turn retries like any transport fault. A `max_tokens` stop
  dispatches nothing either, since the call input may be cut short. Calls that will never run
  (after a failed, stopped, refused or cut-off reply, or queued behind a call that a Stop or
  "Deny and stop" ended) are closed as `error` with `Not run: <reason>`, never left `pending`, so
  the UI and the model's next request both see why. A call whose arguments did not parse as a JSON
  object fails before dispatch.
- A reply the provider's safety filter ended (Anthropic `refusal`, Chat Completions and OpenAI
  `content_filter`, Gemini `SAFETY` and its kin) runs none of its calls and stays `done` with
  `error: "The provider's safety filter ended the reply."`; the UI shows it with finish
  `content-filter`, so a refusal with no text never ends in silence.
- How a `done` reply ended when not by itself is typed: `message.ending` is `length` or `refused`
  (migration 25, which also marks earlier replies by their wording). The UI decides by it, never by
  `error`'s words, which are only for reading.
- An Anthropic content block or delta of a type the adapter does not know (server tools,
  citations, kinds added later) is skipped, not an error: nothing opens for it, so its deltas and
  stop fall on nothing and the reply goes on. Bedrock shares this.
- Gemini tool schemas are adapted, not rejected: `$schema`, `additionalProperties`, `default` and
  `examples` keywords go (a parameter of that name stays), `type: [x, "null"]` becomes `type: x,
  nullable: true`, enum values become strings, and `required` keeps only real properties.
- A reply that stops at its output limit stays `done` but carries `error: "The reply stopped at the
  output limit (N tokens)."`, which the UI shows as `MessageOutputLengthError` with finish
  `length`. The turn ends there.
- Output limit and thinking budget are computed together (`turn::budgets`): the output never
  exceeds the model's own limit; a thinking budget may raise it past the usual 32,000 cap but always
  leaves 1,024 tokens for the answer; a larger budget is reduced to fit, and one that cannot reach
  the 1,024 minimum is dropped. A 32,000 budget on a 32,000-output model sends 32,000 with a
  30,976 budget, never 33,024. A model with no output limit but a known window asks for the same
  reply room (a quarter of the window, at least 1,024), since a server such as vLLM refuses a
  request whose reply could not fit. The composer's model picker marks a model whose window is
  below 16K (`SMALL_CONTEXT`) as too small for most tasks: the system prompt and tools barely fit,
  and compaction cannot help with that.
- **Reasoning levels.** Each catalog model carries `variants`, derived once in `catalog.rs` from
  models.dev's `reasoning_options` (what opencode reads too) and nowhere else: an `effort` option
  becomes one variant per value (`null` is `none`), a `budget_tokens` option becomes `high` (half
  the most) and `max` (the most, capped by the model's output and 31,999). On Claude routes a
  listed budget wins over an effort, because Claude's effort means adaptive thinking, which only
  the newest models accept and they list no budget. Elsewhere the effort wins. A toggle-only model,
  or one with no options, has no variants and no picker.
- A prompt names its variant; the engine looks the name up on the model each request, so a model
  without it asks for nothing rather than failing. The session keeps the last variant a prompt
  chose (`session.variant`, written in the admission transaction alongside the model): a prompt
  that names none, as every prompt the engine sends itself does (worker results, question
  answers), runs at the session's; a prompt that sends `variant: null` clears it. A subagent's
  first prompt and a branch's seed carry the parent's or source's variant, and switching a
  waiting retry to another model may name one (`POST /sessions/{id}/retry {model, variant}`),
  which the session then keeps.
- Each wire sends a variant its own way:
  - Anthropic, Bedrock and Vertex Claude: a budget is `thinking: {type: "enabled", budget_tokens}`;
    an effort is `thinking: {type: "adaptive", display: "summarized"}` plus `output_config.effort`
    (summarized, since those models otherwise return their thinking blank).
  - Gemini: `thinkingConfig.thinkingBudget` or `thinkingConfig.thinkingLevel`, with thoughts included.
  - OpenAI: `reasoning.effort` with an automatic summary.
  - OpenRouter: `reasoning: {effort}` or `reasoning: {max_tokens}`. xAI, Z.ai, LM Studio and
    Ollama: `reasoning_effort`; a budget has no field there, and those routes are never given one.
- Only valid completed blocks are replayed. An aborted message keeps its finished text; its
  unsigned reasoning and any call with unparsed arguments are dropped, along with the
  results those calls would have needed.
- Paths are resolved before anything looks at them: `..` folded, symlinks followed through
  the deepest existing ancestor, verbatim prefixes stripped. Permission asks and the
  read-before-write ledger see the real target. `read`, `glob` and `grep` inside the
  workspace are free; outside it they ask.
- Files that may hold secrets (`tool::sensitive`: `.env` and `.env.*`, `*.env`, `.envrc`, key and
  keystore extensions such as `.pem` and `.p12`, `.npmrc`, `.netrc`, `.git-credentials`,
  `credentials`, private SSH keys) ask to be read even inside the workspace. A name part such
  as `example`, `sample`, `template` or `dist` marks a committed template, which reads freely.
  `grep` skips secrets and says how many it withheld unless one is named as its path, which
  asks first; `glob` and directory listings still show their names. Every read path goes through
  `tool::read_ask`, @ mentions included. The shell is not covered: `cat .env` asks as a command,
  not as a secret read.
- A prompt's files are prepared before it is admitted (`session::attach`), for a new turn and a
  steered prompt alike:
  - An @ mention (a `file:` URL part) is read in as `<file path="...">...</file>` only where
    `read` would read without asking: inside the workspace and not a secret, or allowed by a rule
    or a session approval. Otherwise the file is not read and the model gets a note saying why and
    to use `read`, which asks. A denied path says a rule forbids it. Directories list up to 1,000
    entries; binary files and files that do not exist say so; text past 64 KB is cut at a line
    with the offset to read on from. Only one byte past that bound is ever read from disk. A
    mention read in full is recorded in the session's read ledger, so the model can edit the file
    straight away; one cut short is not, since the model has not seen all of it. The stored part
    keeps the workspace path it was read from (`path`, set only by the engine; one a client sends
    is cleared), so its chip opens the file the way a file link in a reply does, and undo puts it
    back as a mention. The composer sends one part per `@path` that appears whole in the text, so a
    folder passed through while picking a file is not sent with it.
  - Every `data:` URL is taken apart and checked before admission: its MIME must match the part's,
    a text payload must decode to UTF-8 and an image payload must be valid base64. Anything else is
    refused with 400 `attachment` naming the file, never admitted and dropped later.
  - A steered prompt's files are judged against the model the running turn is on (updated when a
    retry switches it), not a model the prompt names, and the prompt is recorded with that model.
  - Text data URLs travel as text. Images go only to a model whose catalog entry says it reads
    attachments. Anything else (audio, video, a PDF sent as data, a non-`file`/`data` URL) and an
    image for a model that cannot read it is refused with 400 `attachment` naming the file, never
    dropped. The UI still extracts PDF, text and CSV attachments to text before sending.
  - `GET /workspaces/{id}/files?query=` serves @ autocomplete: the same walk as `glob` (ignore
    rules apply, version-control internals never), directories with a trailing `/`, ranked by name
    prefix, name substring, path substring, then letters in order.
- `glob` and `grep` never descend into `.git`, `.hg`, `.svn` or `.jj`, and `grep` stops a
  file at its first NUL byte, so binaries produce no matches.
- A mutating call refuses to run if its snapshot cannot be taken or its start cannot be
  recorded, and says so in its result. A result whose save fails is published as an error,
  never as a success the store lacks; a message whose terminal save fails stops the turn.
- Stopping a shell stops its descendants: a Windows job object with kill-on-close, a unix
  process group. Dropping the run future has the same effect as an explicit abort.
- Shell output is captured in bounded memory (`tool::spool`): stdout and stderr in arrival order,
  whole while under 32 KB, then only the first and last 16 KB in memory with everything (up to
  64 MB) in `<data>/tool-output/<session>/<call>.log`. The result names that file and carries
  `outputBytes` and `outputFile`. A timeout or Stop keeps what was printed; the shell tool handles
  Stop itself (`Tool::stops_itself`), so the turn awaits its result rather than dropping it, and the
  call ends `error` with `stopped` or `timedOut` in its metadata.
- PDFs travel the same way: `read` returns one (up to 10 MB, known by `%PDF-`) instead of refusing
  it, and `webfetch` returns an image or PDF URL as the file rather than as text, whatever its
  content type. `webfetch` reads a body a chunk at a time and gives up once it passes 10 MB (or at
  once when `Content-Length` says so), so a huge or endless response never fills memory. A catalog model reads PDFs (`Model::pdf`) when models.dev lists `pdf` among its
  input modalities, or, without them, when it takes attachments on a route whose wire carries a
  PDF whole (Anthropic, OpenAI, Google, Vertex, Bedrock). Each adapter sends its own shape
  (Anthropic `document`, OpenAI `input_file`, Gemini `inlineData`, Chat Completions `file`); a
  model that does not read PDFs gets a line instead. Files share the ten-file and 20 MB budget.
- Images reach the model (`tool::image`). `read` returns a PNG, JPEG, GIF or WebP (up to 5 MB,
  known by its bytes) as an image rather than refusing it as binary, and an MCP result's image
  content is kept instead of becoming `[image png]`, if it is one of those four formats within
  5 MB (an SVG or BMP is named, never sent); text resources are inlined, binary ones named. The
  turn moves the bytes into the content-addressed `blob` table (migration 21) as the call settles,
  so the part, its events and every transcript load carry only `images: [{mime, hash}]`. Each blob's
  messages are recorded in `blob_ref` (migration 22) in the same transaction; a fork copies its
  messages' references and a deleted message takes them with it (cascade), so maintenance drops
  unnamed blobs with an indexed lookup, never a scan of every part. Migration 23 backfilled the
  references for images stored before `blob_ref` existed, from the hashes in tool-call metadata. The request replays them after all of that
  turn's call results ("The <tool> call (<id>) returned this:" then the image), since providers
  want results first. `llm::prepare_images` loads them when a request is built and, newest first,
  keeps them until 10 (`MAX_IMAGES_SENT`) or 20 MB of image data (`MAX_IMAGE_DATA_SENT`) is
  reached, since providers cap both a request's images and its size (Anthropic: 100 images, 32 MB)
  and a rejected request would replay the same images forever; older ones become a line. A model
  whose catalog entry does not take images gets a line for each, on turns and engine requests alike.
- While a command runs, every 500 ms that it has printed more, its part is republished with
  `metadata.output` = the last 4 KB so far (`Spool::recent`), through the call's `tool::Progress`.
  That is shown, never stored: the saved part is the result, and the UI shows the result once the
  call ends, failed ones included. Progress goes out as a transient event
  (`Hub::publish_transient`): it takes a `seq`, so clients keep their order, but it never enters
  the replay window, so a noisy build cannot push real events out and force a resync. A cursor is
  stale only when an event after it was evicted, so the gaps transient events leave are safe.
- Every tool result reaches the model within 64 KB (`tool::spool::MAX_RESULT_BYTES`). `read`
  pages within it by itself (whole lines, at least one per page, with the offset to continue from)
  and a directory listing shows 1,000 entries and counts the rest. Anything else past the bound
  (MCP results, skills, fetched pages, tools yet to come) is cut to its first and last 16 KB in
  `run_call`, with the whole result in `<data>/tool-output/<session>/<call>.result.log` and
  `metadata.resultFile` pointing at it. A session reads files under its own
  `<data>/tool-output/<session>/` without asking (paths resolved first, so `..` cannot leave it);
  other sessions' output and the rest of the data directory ask like any path outside the workspace.
- Background processes do not outlive the call. Once the shell exits, output still in flight gets
  500 ms; a pipe still open after that is held by a background descendant, so the call finishes
  with the shell's exit code, the descendants are stopped, and the result says so. A command with
  no time limit therefore cannot wait forever on a background process's inherited output.
- Prompt admission is one transaction (`Store::admit_prompt`). If it fails, the session's
  busy reservation is released and nothing half-written remains. The UI keeps a prompt's
  submission id until the engine answers for sure: a resend of the same draft after a lost
  answer (a network failure or a 5xx) reuses it and gets the original receipt if the first
  attempt landed; success or a 4xx refusal ends it, and a changed draft gets a new id.
  `Prompt.submissionId`  is
  optional and durable: the `submission` table records id, session, message and a hash of
  the payload. Resubmitting with the same id and payload returns the original receipt, even
  after a restart; the same id with a different payload or session is a 409. The id is checked
  again inside the admission transaction, so two requests racing with one id write one message:
  the loser gets the winner's receipt, or 409 if its payload differs. The UI sends a fresh id with
  every prompt.
- Only `done` and `aborted` assistant messages are replayed to the model. `error` and
  `streaming` rows stay in the transcript as audit history and never enter a request.
- Token refresh is single-flight per provider and fenced: the first turn to notice an
  expired token refreshes it, later turns wait and reuse the stored result, and the refreshed
  pair is written only if the stored credential is still the one the refresh started from
  (`Credentials::replace_if`). Every credential mutation (`set`, `remove`, `replace_if`)
  holds the same lock, so a sign-in or sign-out during the refresh wins and is never undone.
- A sign-in the provider refuses (401/403 on an OAuth token our clock still thinks is live: revoked
  or expired early) is renewed the same single-flight way, once, before any output, and the
  request is sent again on the new token, which the turn keeps for its later steps. A newer stored
  token (another turn renewed, or the user signed in again) is used without a refresh. If renewal
  fails, the reply's error is the provider's words plus "the sign-in has expired and could not be
  renewed; sign in again under Settings > Providers"; an admission-time refresh failure is 401
  `signin_expired` with the same text. A refused key keeps the provider's words
  ("the provider refused the credentials: ..."); "no credentials for this provider" now means
  there was nothing to send.
- The socket client never advances its cursor on a failed hydrate; it retries and keeps
  holding events. A `resync` that lands mid-hydrate folds into the same run. A `hello` from a
  different engine instance, or `close()`, discards held events and disowns any hydrate in
  flight. The app's hydrate (`hydrateFrom`) rejects when any load fails, and it clears every
  cached transcript first so open views refetch after a resync.
- Session listings page by `(updated_at, id)` keyset until a short page, so equal timestamps
  never skip rows; a snapshot is only authoritative when complete. Each listed session
  carries `running` and the client collects it across every page before setting status.
- Tool calls keep the model's order: a run of consecutive read-only calls executes together,
  a mutating call waits for everything before it, and reads after it wait for it.
- `DELETE /sessions/{id}` is the real purge (cascades messages, parts, todos, submissions). It
  takes the session's subagent sessions with it, at any depth; spawned threads are independent and
  stay. The archive purge (`?archived=true`) does the same in one write.
  `PATCH { archived: true }` only archives. The UI's purge coordinator calls delete and
  reports success only when the engine confirms.

Open, deliberately: one credential slot per provider (an API key and a subscription sign-in
replace each other; account profiles are M3 work), and the shell's `session_meta` archive
table still exists beside the engine's `archived_at` until M4 folds shell tables in.

## M2 behaviour

- **MCP.** Servers live in the engine's `mcp_config` table (`PUT /mcp/{name}` with a stdio or
  http config). There is no approval step: a saved, enabled server connects at once, at startup
  and on demand through rmcp (an earlier approval gate was removed; migration 20 drops its column
  and key). Their tools join
  the registry as `<server>_<tool>` (`mcp::tool::wire_name`: any character outside
  `[A-Za-z0-9_-]` becomes `_`, a name past 60 characters is cut, and any name that had to change
  ends in a hash of the original, so `a.b` and `a_b` stay apart; 60 leaves room for the
  subscription route's `mcp_` within providers' 64, so one odd tool name cannot get every request
  refused). Which server a tool came from is asked of the tool (`Tool::server`), never read back
  from its name; tools the server marks read-only run without asking,
  the rest ask under kind `mcp` with pattern `<server>/<tool>`, and "always" therefore
  covers the whole server. Every save, disable, disconnect and remove bumps the server's
  generation; a connect that began under an older generation closes what it opened and
  publishes nothing.
- **MCP sign-in.** A remote server whose connect is refused with a 401 or 403 reports
  `needsSignIn`, judged from rmcp's typed error (`ClientInitializeError::is_authorization_required`)
  or, for HTTP+SSE, the stream's status, never from the error's wording. `POST /mcp/{name}/signin`
  returns `{url}` for the browser. rmcp does the OAuth work (protected-resource and
  authorization-server metadata, PKCE, token exchange and refresh); `mcp::oauth` serves the one-shot
  loopback callback on `127.0.0.1:<random>/callback` (waiting up to 10 minutes) and keeps the tokens
  as a keychain secret `mcp:<server>`, outside the provider list. Once the browser comes back the
  server connects, and every later connect uses the stored tokens, refreshed as needed; the status
  then says `signedIn`. `DELETE /mcp/{name}/signin` forgets them and reconnects signed out.
- Drift registers itself as "Drift" where the server allows dynamic client registration. A server
  that does not takes a pre-registered app in its config, `oauth: { clientId, clientSecret?,
  scopes? }` (upstream's shape): the sign-in uses that client id, sends the secret with the code
  for a confidential app, and asks for the scopes given, else what the server's metadata names. The
  store keeps the client id with the tokens but not the secret, so each connect sets the config's
  secret again before a refresh. Clients see `oauth` as `{ clientId, hasSecret, scopes }`; a
  `null` secret on save keeps the saved one for the same client id, an empty one clears it.
- A sign-in that does not finish (the browser returns an OAuth error such as `access_denied`, the
  token exchange fails, or ten minutes pass) is published as an `mcp.updated`: the server shows
  "Sign-in did not finish: <reason>" and still asks to sign in. The browser tab says the same.
- HTTP+SSE servers sign in too: rmcp's authorized client speaks only streamable HTTP, so a signed-in
  SSE server gets its access token, refreshed when due, as an `Authorization` header on the stream
  and on every message it posts.
- A rename carries the sign-in to the new name; a remove forgets it; a save that changes the URL or
  the app's client id forgets it, so tokens are never sent to another host or used by another app.
  The callback only works on the engine's host (see Sign-in below).
- **MCP transports and limits.** A server is `stdio` (with an optional `cwd`), `http` (streamable
  HTTP) or `sse`, the 2024 HTTP+SSE transport, which the spec has deprecated and rmcp no longer
  ships, so `mcp::sse` speaks it: a long-lived GET whose `endpoint` event names where to POST,
  replies arriving as `message` events. It stays for servers that still need it; nothing new should
  use it. A stdio command is looked up on the PATH as it is now (`platform::process::current_path`):
  Drift's own PATH, then directories Windows has saved for the user and the machine since Drift
  started, so Docker, uv or Node installed while Drift runs are found without a restart. The
  server, the shell tool, checks and formatters all start with that PATH unless their own
  environment sets one; a command found nowhere fails as "<command> was not found on PATH".
  Any of them may set `timeoutSeconds`: a tool call that runs longer fails, saying it may or
  may not have taken effect. Both fields are left out when unset, so older configs keep their hash.
  Each status carries `transport` (`stdio`, `streamable_http`, `sse`) and, once connected, the
  `protocol` version the server agreed to and its `era`; the MCP menu shows all three on each row
  ("Streamable HTTP · 2026-07-28 · stateless"). The era is a protocol version, not a transport, so
  `Transport` gains nothing for it.
- **MCP eras.** MCP 2026-07-28 drops the handshake and the session: no `initialize`, no
  `Mcp-Session-Id`; every request carries its protocol version, client info and capabilities in
  `_meta`, and the server answers `server/discover`. Over HTTP each request is one POST (headers
  `MCP-Protocol-Version`, `Mcp-Method`, `Mcp-Name`, and `Mcp-Param-*` for parameters the tool's
  schema marks `x-mcp-header`), answered as JSON or a stream that lasts only for that request; there
  is no GET stream and no resuming. rmcp 3.5 does all of this; Drift chooses the lifecycle
  (`mcp::lifecycle`, `mcp::attempts`). An HTTP server whose era is unknown is probed with rmcp's
  `Auto`: `server/discover` at 2026-07-28, and on a refusal (a JSON-RPC error, or a 4xx that is not
  401 or 403) or 10 s of silence the `initialize` handshake offering 2025-11-25, on the same
  connection. A stdio server is not: rmcp's 10 s is fixed, and a server still being pulled or
  installed on its first start (a Docker image, an npx download) answers the probe after it, by
  which time `initialize` has been sent over the answer; a 2026 server then refuses it as a
  duplicate. So stdio probes alone (`Discover`) within the 30 s start limit and, refused or silent,
  starts a fresh process for the handshake, which every 2026 server also accepts. HTTP+SSE servers
  predate the probe and always use the handshake.
- The era found is kept on the server's row (`mcp_config.era`, migration 24) and a reconnect starts
  in it (`Discover` or `Initialize`), so probing and the extra start are paid once per config. A
  save forgets it, and it is only written while the row still has the config it was found under. A
  connect in the kept era that the server refuses tries the other era (stdio) or probes (HTTP) at
  once and keeps the new answer; one that only timed out is not retried, so a stuck server is not
  waited on twice.
- Drift answers no server-to-client requests: sampling, elicitation and roots are declined (the
  roots list is empty). A 2026-07-28 server asks for them in an `input_required` result; rmcp
  answers with the refusal and retries, and a server that keeps asking past rmcp's 10 rounds fails
  the call with "the server kept asking for input Drift does not give".
- **MCP connections.** A stdio server and a pre-2026 HTTP server keep a connection open (the
  process, or the session's GET stream) and are watched as below. A stateless HTTP server holds
  nothing open between calls, so it is not watched: a failed request is the only sign of trouble.
  A read-only call whose POST failed is asked again once on the same client; one that may have
  changed something is not, and the model is told so. Only when the client itself has ended does
  the call trigger the reconnect the watch would have.
- **MCP list changes.** A tool list that gives `ttlMs` (required from 2026-07-28) goes stale after
  it. When a turn is planned, each server whose list is stale is listed again (one short request,
  in parallel, at most 2 s each); a changed list publishes `mcp.updated` and the turn sees it. A
  failed re-list waits 30 s before the next. Lists without `ttlMs` are read once at connect. Tools a
  running turn holds are checked against the current list as for a reconnect (`behaves_alike`), so
  a re-listed tool that changed its input or safety hints is refused for the rest of that turn.
  `subscriptions/listen`, the opt-in stream of change notices, is not opened: it would hold a
  connection open per server for what the TTL already covers.
- **MCP resources and prompts.** While a connected server declares resources, turns are also
  offered `mcp_resources` (list, every such server or one) and `mcp_read_resource` (server and
  uri; text inline, image and PDF blobs as files the model looks at, other binaries named). Both
  only read, so neither asks. A server's prompts (listed at connect when it declares them) become
  slash commands named `server:prompt` in `GET /workspaces/{id}/config`, with their arguments as
  `arguments` (the UI's usage hint); `POST /sessions/{id}/command` has the server fill one through
  `prompts/get`, the typed words going to its arguments in order, the last taking the rest, and
  submits the text of its messages (502 `mcp` when the server fails).
- **MCP lifecycle (M3).** A turn's tools come straight from the connected servers when it is
  planned (`Engine::offered_tools`); there is no separate copy to fall behind. Planning waits up to
  2 s (`READY_WAIT`) for connects and reconnects already under way, so a server starting at the same
  moment is not briefly missing; a slower one joins later turns. Each connected server is watched
  (every second) for a transport that closed by itself, such as a stdio server that exited. It is
  then dropped from later turns, shown connecting, and reconnected with waits of 500 ms doubling to
  30 s, reading its row afresh each attempt; it stops when it connects or when its generation
  changes (save, disable, disconnect, remove), so a deliberate disconnect is never undone and a
  reconnect started under an old definition is discarded. A tool's own failure (`isError`) is not a
  lost connection. Saving an enabled server reconnects it at once (reload).
- **MCP tools in running turns.** A server has one slot from enable to disable or remove, and every
  tool object made from it holds that slot and the client it was planned with. A call uses the
  slot's current client when it serves the same definition (same config hash), so a running turn
  follows a reconnect or reload instead of calling a dead process; otherwise it uses its own client,
  so saving a different command never moves a running turn onto a command it was not planned with.
  That client stays alive while the turn holds it.
- A call cut off by a lost connection is never replayed if the tool may change something: the
  model is told the call may or may not have taken effect and was not retried. A read-only tool is
  asked again, once, of the reconnected server if one comes within 10 s.
- Disable and remove close the slot: calls under way end with an error saying the effect is
  uncertain, every client the slot served is closed and its process tree killed, including ones
  running turns still hold, and tools captured before the disable refuse from then on. Re-enabling
  makes a new slot; tools captured before stay refused. A disconnect only takes the server out of
  later turns, like a save.
- **MCP lifecycle coordination.** One lock (`Servers::slots`) orders everything that decides which
  connection a server has: a save, disable or remove writes its row and bumps the generation in the
  same step (`Servers::change`); a connect reads the row and the generation together when it starts;
  and a finished connect publishes its client only if it is still the newest attempt at that
  generation. A connect therefore never pairs a new generation with an old row, or the reverse.
- Each connect is an attempt with its own cancel token. A user's connect for the same server, or any
  change to it, cancels the one in flight at once rather than letting it run to its timeout. The
  engine's own connects (the startup sweep, reconnects) never cancel anything: they skip a server
  that is live or already connecting, checked under the same lock as the start.
- The startup sweep (`connect_all_mcp`) begins every enabled server's connect at once, each in its
  own task, before the engine fetches the model catalog, so a server that is down or slow to start
  holds up no other and nothing else. It returns once every attempt is registered, so turns planned
  meanwhile wait for them (`READY_WAIT`, 2 s) as for any connect. Recovering interrupted tasks waits
  at most 15 s (`RECOVERY_WAIT`) for servers still connecting, then goes on without them.
  Starting (handshake or discovery) and `tools/list` each get 30 s; a server that misses either fails with a message
  saying which. A stdio server is adopted into a process tree (job object on Windows, process
  group on unix) as soon as it spawns, so a cancelled, timed-out or replaced attempt, or a dropped
  connection, takes the server's children with it. However an attempt ends, even when its caller is
  dropped mid-flight, a guard clears its record and wakes turns waiting in `wait_ready`, so nothing
  is left showing connecting. The start limit covers the handshake or discovery; a first connect
  that must probe gets 10 s more.
- Reconnect backoff carries across connections that drop again quickly: a server that crashes
  straight after each reconnect is retried at 500 ms, 1 s, 2 s and so on up to 30 s. Only a
  connection that held for a minute starts its next reconnects from 500 ms again.
- **Config.** `Config::load` reads `~/.config/drift/drift.json` then `<workspace>/drift.json`
  (project rules first, so they win), plus `.drift/agents/*.md`, `.drift/commands/*.md` and
  skills from `.drift/skills`, `.agents/skills` and `.claude/skills` at both roots (project
  shadows home). Instructions, general first: `~/.config/drift/AGENTS.md` (else
  `~/.claude/CLAUDE.md`), then each directory's file from the repository root (nearest `.git`)
  down to the workspace, then what drift.json lists; in each directory `AGENTS.md` beats
  `CLAUDE.md`. A subdirectory's file below the workspace is not in the system prompt: the first
  `read` of a file under it appends it as a `<system-reminder>` (outermost first), once per
  session and again after a compaction summarises the read away. Reminders take at most half the
  result, the page fits in the rest, and one that does not fit is named for the model to read. `GET /workspaces/{id}/config` serves the
  merged result. Front matter is `key: value` lines; `tools` may also be `[a, b]`, `- item` lines,
  or a `name: true|false` map (opencode's shape: `false` takes that tool away from all the rest,
  `true` changes nothing). `tools: []` means no tools; leaving it out, or `tools: {}`, means every
  tool. Tool names match in any case, so Claude-style `tools: Read, Grep` works. `drift.json` may hold `//` and `/* */`
  comments and trailing commas (`config::jsonc`). One that still cannot be parsed is named in
  `Config.problems`, and every turn in that workspace refuses with 400 `config` until it is fixed,
  because running without its rules would drop its denies; the UI shows the problem when the
  workspace config loads.
- **Agents.** `build` and `plan` are built in; `plan` gets only read-only tools and its
  prompt. A project agent of the same name replaces a built-in. A session's `agent` is set
  on create or `PATCH`; the agent's prompt is appended to the system prompt, its `tools`
  list filters the registry, its `model` is the default when the session has none. The
  filtered set is pinned for the run: a call to any tool outside it is refused before
  permission, snapshot or dispatch, so plan mode cannot write even if the model asks.
- Settings overrides an agent with exactly what the engine applies (`AgentOverride`): `prompt`,
  `model` (`provider/model`, or empty to inherit), `steps` (its own step limit) and `tools` (the
  tool names it may use). The shell refuses to store any other field, naming it, and the editor
  shows the agent as the engine resolved it, in those fields only. An override's tool list is
  never empty, because on an agent an empty list means every tool: that would quietly lift
  `plan`'s read-only set. Reset restores the definition's own tools. Stored overrides from
  before keep only these fields when next saved.
- The composer shows, for agent, model and reasoning level, an unsent edit (a pick made in this
  browser and not yet sent), else what the session runs as next (what waits, else what the engine
  saved, and the session's model), else the global default. An accepted send clears the edits,
  so what is shown is what the engine runs. A prompt names its agent and level only when they
  differ from what the session runs as next, so an ordinary follow-up never names them and
  steers into the running turn. A level the chosen model does not offer is left out (the session
  keeps its own), never sent as null; null is sent only when the user picks Default.
- A prompt may name its `agent`; the composer sends it when it changes the session's. It must be a
  primary agent of the workspace (a subagent, an action or an unknown name is 400 `agent`),
  it is written to the session in the admission transaction, and that turn and every later
  one run as it until a prompt names another. Each message records the agent it was written
  under (a turn's replies the agent that turn runs as), so history keeps it after a switch.
- **Commands.** `POST /sessions/{id}/command` expands the template (`Command::expand`) and submits
  the result as a turn: `$ARGUMENTS` is everything typed, `$1`..`$9` one word each with the highest
  taking the rest, and a template with neither gets the arguments appended rather than dropped.
- **Skills** are listed in the system prompt by name and description; the `skill` tool
  returns SKILL.md's body and its directory.
- **Formatters.** After a mutating tool succeeds, the first formatter whose extensions
  match each written file runs. Built-ins (prettier, rustfmt, gofmt, ruff, black) apply
  only when installed, in the project's `node_modules/.bin` from the file's directory up (where a
  devDependency prettier lives) or else on PATH as a shell finds it (npm's `prettier.cmd`
  included), and only where the
  project uses them, looked for from the file's directory up to the repository root: prettier when
  a `package.json` names it or a prettier config exists, ruff with `ruff.toml` or `[tool.ruff]`,
  black with `[tool.black]`, rustfmt with `rustfmt.toml`; gofmt always. A global prettier never
  reformats a project that does not use it. rustfmt reads the file on stdin at the crate's edition
  (from the nearest `Cargo.toml`) and its output replaces the file through the staged writer, as
  every whole-file write the engine makes does, so the out-of-line modules it
  would otherwise follow are never touched. `drift.json` `formatters` can set a name to `true`
  (on without looking), `false` or `{ command, extensions }` with `$FILE`. Results land in the
  call's `metadata.formatted`; failures are ignored.
- **Checks.** The checks in `drift.json` `checks` run once per step, after all of the step's
  calls, over every file its edit, write and apply_patch calls wrote (a shell command lists no
  files, so it is not checked). Each is `{ command, extensions }`, or `false` to turn off one an
  earlier file set; there are no built-ins, because a linter or type checker after every step is a
  cost the user chooses. A command naming `$FILE` runs once per matching file (`eslint $FILE`); one
  without runs once for the step (`tsc --noEmit`), however many files it wrote. Runs go four at a
  time, each for at most 60 s, all within one 90 s budget for the step; a run the budget cuts off is
  `unavailable`. The program is found on PATH as a shell would, `.cmd` and `.bat` shims included,
  and runs in the workspace. A non-zero exit adds its output (at most 4 KB per run) to the step's
  last writing call, under "Checks reported problems after this step's changes", labelled by check
  and file, before the model's next request; a pass says nothing, and a check that cannot start or
  runs out of time is not mentioned to the model. Output identical to what the same check said last
  time in the session is named ("the same problems as reported before"), not sent again, so
  problems already in the repository do not fill the context step after step; a pass forgets it,
  and so do compaction and undo, after which the model may no longer have the full report.
  A check that changes a file the step wrote (a fixer such as `eslint --fix`) is announced as a
  formatter is ("A check then changed ..."), with the files in `metadata.checkChanged`. The files
  are captured around the checks as a writing call's are, and what a check rewrote is appended to
  the last writing call's change record, so undo chains it after the step's writes and puts the
  file back to before the step, rather than keeping it as someone else's edit.
  - The capture spans the whole check run (up to the 90 s budget), far longer than a file tool's.
    A change in that window to a file none of the checks runs over (by extension) cannot be a
    check's, so it is recorded as observed and undo leaves it alone, as it does what a shell
    command was seen changing. A change to a file a check does cover is taken as the check's:
    if the user or another session edits such a file while its checks run, undo puts that edit
    back with the rest of the step. Nothing in the file can tell the two apart.
  - A check whose command names no `$FILE` (`eslint --fix .`, `cargo fmt`) may rewrite anything,
    so when one runs the whole tree is captured, as for a shell command. Its changes to files the
    step wrote and a check covers are the check's, as above; any other file that changed is
    observed, named to the model ("files this step did not write changed too"), kept in
    `metadata.checkObserved`, and left as it is by undo.
  - When the files cannot be captured before the checks, their bytes are compared instead: a
    rewrite is still announced, listed in the call's `unrecorded`, and the result says undo cannot
    put it back. Every run
  is in `metadata.checks` (`check`, `status` of `passed`, `problems` or `unavailable`, `output`).
  Stop cuts the checks off with their whole process tree; the writes stand. Checks stand in for
  LSP diagnostics until those land (M4).
- **Project commands.** A command a check or formatter names in the project's own `drift.json`
  runs only once the user has allowed it; the user's `~/.config/drift/drift.json`, built-in
  formatters and a project's `false` need no say-so (as custom providers come only from the user's
  file, opening a cloned repository must not run what it names). The first write one of those
  commands covers (by its extensions) asks once, a permission card of kind `project-commands`
  listing every such command (`check lint: eslint $FILE; formatter prettier: ./fmt.sh $FILE`); a
  write none of them covers asks nothing. The answer covers the whole set: always is kept for the
  workspace under a hash of all of them (setting `trustedCommands:<workspace>`), so any change asks
  again; once allows them for the session; deny skips them for the session, built-in formatters and
  the user's own commands still running. Subagents take the answer of the session that delegated
  to them, along the same lineage as permission approvals, so a delegated task does not ask again.
  A permission rule of that kind (pattern `*`) allows them without asking.
- **Permissions** resolve in order: session "always" answers, the workspace's `drift.json`
  rules, then the global policy.
  - File asks (read, edit, write, apply_patch) carry the absolute path and, inside the workspace,
    the relative one (`Ask::path`); rules and approvals match either, so a committed `src/**`
    or `src/generated/**` rule works on every machine.
  - A subagent inherits its parent's session approvals (`Permissions::inherit`, registered when
    `task` creates it). One way only: an approval given inside the worker stays with the worker,
    and branches inherit nothing.
  - Replies are `once`, `always`, `deny` and `stop`, with an optional `message`. `deny` refuses
    the call and the turn goes on; the model's result reads "The user denied permission for this
    call. They said: ..." when there is a message. `stop` refuses it and ends the turn that asked,
    as Stop does for that turn only: for a subagent's request that is the subagent (its parent sees
    it stopped and goes on), never the parent or other workers. A call a rule refuses says so
    ("A permission rule forbids this call."), never that the user did. The permission card offers
    only Allow, Always and Deny; the engine keeps `stop` and `message` for other clients.
  - A shell line is split into the simple commands it runs (`tool::command`, bash and PowerShell
    quoting, escapes and operators) and each is judged on its own: any denied command denies the
    line, and it runs without asking only if every command is allowed. So `git *` does not cover
    `git status && rm -rf ~`.
  - A line that hides what it runs (command substitution, backticks, subshells, groups, script
    blocks, `eval`/`Invoke-Expression`, the `&` call operator, or a launcher such as `bash -c`,
    `sudo`, `env`, `xargs`) is judged whole, and a wildcard rule never allows it, not even `bash *`
    in drift.json or an "always" widened to a pattern: only an exact rule or approval of that line
    allows it, while any matching deny still denies it.
  - A redirection that writes a file (`>`, `>>`, `>|`, `n>`, `&>`, `&>>`, `>&file`, `<>`, and
    PowerShell's `*>`) makes the line exact-only too: `git status > victim.txt` is covered by
    neither `git *` nor an "always" for `git status`, and "always" on it records only that line.
    Stream duplications (`2>&1`, `>&2`, `n>&-`) and sinks (`/dev/null`, `/dev/stdout`,
    `/dev/stderr` in bash, `$null` in PowerShell) write nothing. Git Bash has no `nul` device, so
    `> nul` counts as a write; the tool description on Windows says the shell is Unix bash, to use
    `/dev/null` and not `NUL`, `cd` and not `cd /d`, and `C:/dir` or `/c/dir` paths. Quoted or escaped operators are text. Redirections are listed after
    the command's words (`Ask.writes` names the targets) so the program word stays first.
  - A secret read is held to the same bar: `read *` or an "always" widened to `**` never allows
    `.env`; a rule or approval naming that file does, and any matching deny still denies.
  - A `cd` (or `chdir`, `pushd`, PowerShell `Set-Location`/`sl`/`Push-Location`) whose target stays
    inside the workspace is dropped from the commands judged: moving around changes nothing, so
    `cd crates && cargo test` asks only about `cargo test`. The directory is followed along the
    chain. A move that leaves the workspace or cannot be read (`~`, `-`, a variable, a glob, a flag)
    still asks, and so does every move after it. Nothing grants directory changes: the ask's
    pattern is still the whole line.
  - "Always" grants each command separately. Known subcommand tools (`git`, `cargo`, `npm run`,
    `docker compose`, `gh`, ...) widen to their subcommand with any arguments (`cargo test` covers
    `cargo test --release`, not `cargo publish`); anything else, and every non-shell target such as
    a path, is granted literally. Nothing widens to a bare program name. Subcommands that run or
    install arbitrary code (`run`, `exec`, `x`, `dlx`, `install`, `i`, `add`, `ci`, `get`, `update`,
    `upgrade`) never widen: `cargo run`, `uv run python`, `docker run`, `npm install` and
    `pip install` are approved exactly as written. `npm`/`pnpm`/`yarn run <script>` still widen,
    since the script is the project's own and is named.
  - Deny rules also see each command as it runs: leading `NAME=value` assignments dropped (bash)
    and PowerShell's built-in aliases spelt as their cmdlets (`rm` is `Remove-Item`, `iwr` is
    `Invoke-WebRequest`, `saps` is `Start-Process`). So `FOO=1 git push` meets a deny for
    `git push*`. Approvals, allow rules and the line the user sees keep the command as written, and
    an assignment or alias never hides a launcher (`X=1 bash -c ...` is still opaque).
- **Sign-in.** Anthropic offers Claude Pro/Max and Console (paste-the-code flows); OpenAI
  offers ChatGPT through the Codex flow, where the engine listens on `localhost:1455` and
  the callback route completes on its own.
  - Loopback callbacks (Codex, MCP sign-in) listen on the host running the engine. Signing in from
    a remote device sends the browser back to that device's own localhost, where nothing listens,
    so those sign-ins must be done on the host; the paste-the-code flows work from anywhere.

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

- `tests/conformance/`: bun tests that spawn the real `drift-engined` (build it first; the gates
  and CI do), point
  `DRIFT_ANTHROPIC_BASE_URL` at a fake that replays recorded SSE fixtures in small chunks,
  and drive the engine over HTTP and WS: tool turn with permission, truncated stream,
  denial, retry plus submission replay, abort, cursor replay and restart. `bun run
  test:conformance` runs them alone; `bun run test` includes them.
- Rust unit tests per module. Edit matcher, `apply_patch` parser, SigV4, PKCE, catalog
  filter, permission rules and WS replay each get their own suite.
- `tests/conformance/`: bun test suites driving the HTTP and WS API against
  `drift-engined` through a recording provider proxy (own crate). Fixtures are committed.
  Upstream's test files are mined for scenarios, never copied as code.
- Perf: the three M0 numbers run in CI and fail the build on regression past a threshold.

## Effect on the app

- `src/engine/` talks only to the native engine. `native/client.ts` wraps the generated
  types, `native/events.ts` runs the socket, `actions.ts` implements every action the UI
  calls. Actions the engine cannot serve yet (share) raise a "not available yet" notice and return the
  neutral value their callers expect; each comes back native in the milestone that owns it.
- `native/adapt.ts` maps native sessions, messages, parts, permissions, providers and events
  onto the legacy store shapes (`@opencode-ai/sdk` types) that `store.ts`, `events.ts` and
  the components were written against. That keeps the whole UI working on the new engine
  without touching a component. At M4 the store adopts the generated types, the adapter
  goes, and `@opencode-ai/sdk` leaves `package.json`.
- Retained features not yet moved to the engine, tracked in `CHECKLIST.md` and not to be read as
  native: the removed-workspace purge (its session removal is a stub that reports nothing
  deleted, so the purge retries forever), transcript search, and Settings > Storage, both of which
  still read OpenCode's database and schema and never see `drift.db`.
- Settings has no Jev tool routing: the native engine routes no tools, so the toggle and its
  status polling are gone. The shell's `tool_routing.rs` remains only for the frozen opencode
  plugin and goes at M4 with the rest of the shell engine glue.
- A finished reply's footer offers Fork from here: `POST /sessions/{id}/fork {atMessage}` copies
  the history through that reply into a new conversation. `/fork` and the sidebar's fork copy
  everything finished.
- Archive and restore go through the engine first: the sidebar button, `/archive` and the
  Archive dialog call `PATCH /sessions/{id} {archived}` (archiving stops the session's turn and
  workers and forgets its permissions), and only once that succeeds is the shell's archive record
  written or cleared. That record still hides the thread and is the seven-day purge tombstone. A
  refused archive leaves the thread in place with an error; a refused restore leaves it archived.
  The purge deletes with `DELETE /sessions/{id}?archived=true`, which removes the session only while
  it is still archived, in one statement, and is 409 `active` otherwise: a thread restored in the
  engine whose shell record outlived the restore is kept, and the record is dropped. Archive,
  restore and the purge also take turns in the UI, so a purge never runs mid-restore.
- Model-family system prompts are not a native feature: the engine sends one Drift base prompt
  (`session/prompts/system.txt`) to every model, plus the agent's prompt. Its environment section
  gives the working directory, whether it is a git repository, platform, date and the model's
  catalog name, and the scratch directory (`tool::scratch_dir`: `Drift` in the system temp
  directory, made when the engine opens), where reading, writing, editing and patching ask
  nothing (secret files still do), so temporary files stay out of the workspace. `task`'s text
  asks the model to say whether a subagent should change code or only report, how to check its
  work, and not to redo work it has handed off. A prompt sent to another agent right after the
  plan agent replied carries a reminder, in the request only, that plan's read-only limits no
  longer apply (`prompt::remind_left_plan`); later turns follow a reply by the new agent, so they
  do not. Each MCP server whose tools the turn offers adds its initialize `instructions`
  under "# Instructions from the <name> MCP server" (`prompt::Setting`). Settings shows the
  family prompts read-only under a notice saying they are not applied, offers no save, and keeps
  Reset only to clear an override stored before. The shell still records `family:*` for the
  frozen opencode plugins; nothing native reads it.
- MCP env and header values are secrets: they go into the engine and never come out. `/mcp`
  and `mcp.updated` carry a `ServerView` with the names only. A
  save sends a `ServerConfigInput` in which a `null` value keeps the one saved under that name
  (400 `secret` when nothing is saved under it), so the editor shows saved values as empty
  masked fields and never holds one. Adding uses `?create=true` and is 409 `taken` rather than
  replacing a server of that name. Renaming is `POST /mcp/{name}/rename {to}`: one store step
  under the lifecycle lock that keeps secrets, closes the old name's slot as a remove would (its
  tools are named after it), and is 409 `taken` if the new name exists. The config's hash (which
  includes secrets) stays inside the engine, where it tells a connection which definition it serves.
- A captured MCP tool runs on a reconnected server only if that server still defines the tool
  as the turn was given it: same name, input schema, and read-only and destructive hints (a
  missing hint counts as MCP's default). A reworded description or a new title does not matter.
  A server that came back with the tool redefined (say, no longer read-only) is refused for that
  call, and the next turn sees the new definition.
- MCP management has one authority, the engine. The manager and the registry installer read
  `state.mcpServers` (loaded on hydrate, kept current by `mcp.updated` and `mcp.removed`) and
  change servers only through `/mcp`: save (a rename is one `POST /mcp/{name}/rename`),
  enable or disable, connect or disconnect, sign in or out, remove. Each row offers, in order,
  delete, Sign in (while the server needs one) or Sign out (while it has one), edit, disconnect (or
  connect) and the enabled toggle; Sign in opens the page in the browser and the row connects by
  itself once the user is back, its status reading "sign-in required" until then. The editor offers
  exactly what the engine's config holds (a command, its arguments, environment and working
  directory, or a URL and headers, plus the transport and a call timeout). A remote server's
  "Sign-in app (optional)" section, collapsed unless one is set, takes the client id, secret
  (masked, kept when left empty) and scopes of a pre-registered app; the sign-in itself is not a
  field, because the server asks for it. Workspace `opencode.json` servers and the shell's
  `mcp_server` and `mcp_decision` tables are no longer read by the UI.
- The MCP registry tab (`src/ui/mcp/registry.tsx`) lists GitHub's MCP registry
  (`api.mcp.github.com/v0.1/servers`, the same API as the official one, curated and ordered by
  stars), read whole once per app run (cached 6 h, every page) and searched locally: each word
  must match the title, the name after its namespace, the publisher, a GitHub topic or the
  description, in that order of weight, stars breaking ties. A search of two letters or more also
  asks the official registry (`registry.modelcontextprotocol.io`, name search only, slow), whose
  matches GitHub lacks (by name or repository) join below under their own heading when they come.
  Entries the registry marks deleted, deprecated or not latest are left out; one package or remote
  Drift cannot read leaves the rest of the entry usable. Cards show the logo, publisher, stars,
  description and how it runs (Remote, npm, PyPI, Docker, Needs a key); All, Remote and Local
  filter them.
- Opening a card shows its install sheet (`registryOptions` in `src/mcp-registry.ts`): every way
  Drift can run it, remotes first, and for the chosen way only what the entry leaves open, as
  fields: a header or variable with no value, or a `{placeholder}` in a URL, header or argument
  (secret ones masked, described from the entry). The first way needing nothing typed is chosen.
  Remotes become `http` or `sse` by the entry's own transport, and a bare key typed into an
  Authorization header is sent as `Bearer <key>`. npm runs as `npx -y <pkg>@<version>`, PyPI as
  `uvx <pkg>==<version>`, both `@latest` or unpinned when the entry names no exact version (as
  READMEs do), which the sheet says; after `uvx --from <source>` the entry names its own command.
  OCI images run as `docker run -i --rm`, with every `-e NAME=value` moved into the server's
  environment and passed by name, so secrets live where the engine keeps them write-only, never in
  the arguments. A named argument with nothing after it is a flag. The server installs under its
  title as a tool-safe name (`github`), else its last name segment. Once saved it connects; if it
  answers that it needs a sign-in, the sign-in page opens at once and the Servers tab shows it.
- The composer's reasoning picker lists the model's catalog variants (`adaptModel` fills
  `variants` from them) and sends the chosen name as the prompt's `variant`, or null for the
  model's default. A model without variants shows no picker.
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
