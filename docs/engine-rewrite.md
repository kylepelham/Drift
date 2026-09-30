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
| Tools | `read`, `edit`, `write`, `apply_patch`, `bash`, `glob`, `grep`, `webfetch`, `todowrite`, `skill`, `question`, `task`, `read_thread`. M3 adds parent-scoped `task_output` and `task_stop` for background workers. Branch creation is never a model tool. |
| Dropped | `websearch`, `lsp`, `execute`, `plan`, share, ACP, TUI, CLI, Jev tool routing, Copilot, Azure, Cohere, Perplexity, GitLab, Venice, Poe, Alibaba, Gateway. |
| Edit | Exact match only, with line ending normalisation on both sides. On a miss, return the closest region so the model can re-read cheaply. `apply_patch` replaces `edit` and `write` for models whose catalog profile says so. |
| Post-edit | Formatter hooks only: built-in table, `drift.json` can add or disable, failures logged and never surfaced to the model. No language servers. |
| Snapshot and revert | Kept. Shell out to `git` with a shadow git dir per worktree. Snapshot before every writing tool. Revert restores a snapshot; diffs are computed between snapshots. |
| MCP | Native `rmcp` (stdio, streamable HTTP, OAuth). Approval, reconnect and reload designed in rather than patched on. |
| Storage | One `drift.db`, one writer, WAL, strict tables. Engine tables live beside the existing shell tables. |
| Config | `drift.json` at the project root, `.drift/{agents,commands,skills}/`, `~/.config/drift/`. Instructions from `AGENTS.md` and `CLAUDE.md`. Skills from `.drift/skills`, `.agents/skills` and `.claude/skills` at project and home. No runtime `opencode.json` fallback. |
| Identity | `DRIFT_*` env vars, `~/.local/share/drift` data dir. A one-time migrator runs on first launch. MIT attribution for opencode stays in `licenses/`. |
| Permissions | Upstream semantics (allow, deny, ask; path globs; session-scoped always; agent overrides) reimplemented once, with a single protocol. |
| Session tree | Shared session storage, distinct ownership: `task` creates a hidden worker in foreground or background; the user branches an independent sibling conversation through a reviewed handoff. A conversation's parent link is provenance, not worker cancellation ownership. See "Subagents and branches" and "Background-worker implementation". |
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
GET    /sessions/{id}/tasks                 owned workers; pending M3
GET    /tasks/{id}                          state, progress and result handle; pending M3
POST   /tasks/{id}/abort                    stop one owned worker; pending M3
POST   /sessions/{id}/compact
POST   /sessions/{id}/fork                  {atMessage?}
POST   /sessions/{id}/move                  {workspaceId}
POST   /sessions/{id}/revert                {messageId}
POST   /sessions/{id}/unrevert
GET    /sessions/{id}/todos
GET    /mcp                 POST /mcp/{id}/connect | disconnect | approve | auth
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
- MCP through rmcp: stdio and HTTP, approval; reconnect/reload are M3 lifecycle work.
- `todowrite`, `skill`, blocking `question`, `webfetch`. Async questions are M3 work.
- Formatter hooks.
- Config loading: `drift.json`, agents, commands, skills, instruction files.

### M3: tree and lifecycle

- `task` foreground and background subagents; durable launch/completion delivery,
  bounded supervision, attributed approvals and restart interruption handling.
- User branches with a reviewed handoff; `read_thread`. Background workers remain
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
| Context | A self-contained delegation prompt | A reviewed summary and excerpts, with the source cutoff recorded |
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
  reply or a summary. Either way the call keeps
  `metadata.sessionId` and `metadata.outcome` (`replied`, `failed`, `stopped`) for drill-down.
  Listings include subagent records for inspection; the sidebar shows only
  active/awaiting-attention workers. Background mode below is pending, not claimed implemented
  by this foreground path.
- Delegation is one level deep: subagents are never offered `task` or `read_thread`, the tools
  refuse to run from one, and a subagent cannot be branched from.
- The model cannot create branches; there is no `spawn_thread` tool. It may suggest one in prose.
  `read_thread` lets a conversation check on a branch taken from it when the user asks.
- Branching is two calls. `POST /sessions/{id}/branch/draft {goal}` sends the source transcript up
  to its last finished message, plus a handoff instruction, to the source's model: one request, no
  tools run, nothing stored. It returns `{goal, title, summary, excerpts, cutoff}`. The user edits
  that in the review dialog, then `POST /sessions/{id}/branch` creates the session with
  `branch_cutoff`, seeds its first message with the handoff and starts it on its own abort token.
- A branch is not a fork; see below.

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
| `handoff` | action | drafting the context a `/spawn` branch carries | the source conversation's |

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
    used at least `context - min(output limit, 32k)` tokens. The UI's context meter uses the same
    sum (`contextStats` in `src/engine/store.ts`), so "until compaction" is where it happens. One
    attempt per step; a failure still lets the request go.
  - Overflow: a provider error recognised as too long (`llm::Error::is_context_overflow`, status
    400 or 413 plus each provider's wording) compacts and retries once per turn; a second overflow
    fails the turn.
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
- Every provider, OAuth, catalog and MCP HTTP request goes through one shared client
  (`llm::http::client`): shared connection pool, 15 s connect timeout, TCP keepalive. A provider
  request whose response has not begun within 120 s fails as a transport error, and a stream with
  nothing at all (not even a ping or comment) for 300 s ends as a stalled stream; both retry like any
  transport failure. Reasoning models can think silently for minutes, hence the long idle limit;
  both limits are per adapter (`Timeouts`). Stop ends a turn while its request is still being sent
  or waiting for the response to begin, not only once it streams.
- Each wait publishes `session.retry { attempt, message, nextAt }`, which the UI draws as the retry
  notice with a model picker; `session.status running` follows when the wait ends. Stop ends a wait
  at once.
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

#### Undo and redo

- Every writing call records what it changed as `metadata.changes: [{path, before, after}]`, blobs
  in a shadow git dir under the data dir (`session::changes`). File tools name their paths
  (`Tool::touches`), so exactly those files are recorded before and after the call, after
  formatters run. A shell command can change anything, so the tree (the workspace's `.gitignore`
  applies) is compared just before and after it. That comparison shows what changed, not who changed
  it: the user, an editor or another session may have written during the command. Those changes are
  recorded with `observed: true`, and undo and redo never apply them; they are listed as
  `unattributed` and shown in their own notice. A path whose recorded history includes any observed
  change is left alone as a whole. A call whose files cannot be recorded does not run.
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
- `POST /sessions/{id}/revert { messageId }` takes a prompt the user sent. It hides that prompt and
  everything after it (`session.revert.messageId`, the UI filters) and puts each file the hidden
  turns changed, subagents included (ids are time-ordered, so calls sort across sessions), back to
  its state before the first of those changes. Only if the file still holds what the session last
  wrote: anything changed since is kept, never overwritten, and listed (`kept`, shown as a notice).
  No other file is read or rewritten. Calling revert again moves the point: back undoes the range
  in between, forward redoes it, with the same check.
- `POST /sessions/{id}/unrevert` redoes the hidden range the same way and clears the marker.
- The shadow repo is one per workspace, shared by every session and subagent in it, so creating it
  and every index operation (tree captures, prunes) hold a per-workspace lock.
- Files over 10 MB are never copied into it: a file tool refuses to change one, since the change
  could not be undone, and tree captures leave them out through the shadow repo's own exclude file
  (the workspace's `.gitignore` is untouched). Exclusion only stops untracked files, so each capture
  also removes oversized paths from the shadow index before adding: a file recorded while small that
  grows past the limit never enters the object store. Each capture carries the oversized paths it
  left out with their size and mtime, and a path oversized on either side of a comparison is
  reported in `metadata.unrecorded`, never as a deletion or creation, so undo cannot delete a large
  file or restore a stale small copy over it. An oversized file left untouched is not reported.
- Retention: `Engine::maintain` runs at startup and every six hours while the engine is up (it
  holds only a weak reference, so it never keeps an engine alive). Each pass, `prune_snapshots`
  pins every blob a stored call's `changes` refers to (archived sessions included) under a private
  ref and prunes the rest, sparing objects younger than two hours so a capture in flight keeps its
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

Initial async mode is selected at launch. Foreground-to-background promotion,
agent teams, arbitrary cross-agent messaging and automatic post-crash execution
resume are not required for the first implementation. `/spawn` retains its existing
tool-free draft/review path and independent session lifetime.

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
- Anthropic prompt caching uses all four breakpoints: the last tool, the system prompt, and the
  last cacheable block of the two newest user messages. The newest writes the whole prefix; the
  one before it sits exactly where the previous step wrote, so a tool loop pays only for each
  step's new blocks even past the 20-block lookback. Thinking blocks and empty text never carry
  one. The marks are set in the adapter's shared body, so key, subscription (whose extra system
  blocks add none) and Anthropic-dialect gateway base URLs all cache alike.
- OpenRouter is not covered. Its Chat Completions route caches Claude only with explicit
  `cache_control`: a top-level `cache_control` for automatic caching, or per-block breakpoints
  (at most four) on Anthropic-compatible upstreams (OpenRouter prompt-caching guide, checked
  2026-09-30). The compat adapter sends neither. OpenRouter is not a catalog provider, so the route
  cannot be selected yet; when it joins the catalog, the choice between the two forms should come
  from its catalog entry and be verified with a recorded exchange before gateway caching counts as
  done.
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

- Terminal contract for a streamed response: the engine requires a stop reason (Anthropic
  `message_delta.stop_reason`, OpenAI `response.completed`/`response.incomplete`, Gemini
  `finishReason`, Chat Completions `finish_reason` then `[DONE]`). `message_stop` alone is
  not completion. A stream that ends without one is an error, not a completed message: its
  tool calls never run, and the turn retries like any transport fault. A `max_tokens` stop
  dispatches nothing either, since the call input may be cut short. Calls that will never run
  (after a failed, stopped or cut-off reply) are closed as `error` with `Not run: <reason>`, never
  left `pending`, so the UI and the model's next request both see why. A call whose arguments did
  not parse as a JSON object fails before dispatch.
- A reply that stops at its output limit stays `done` but carries `error: "The reply stopped at the
  output limit (N tokens)."`, which the UI shows as `MessageOutputLengthError` with finish
  `length`. The turn ends there.
- Output limit and thinking budget are computed together (`turn::budgets`): the output never
  exceeds the model's own limit; a thinking budget may raise it past the usual 32,000 cap but always
  leaves 1,024 tokens for the answer; a larger budget is reduced to fit, and one that cannot reach
  the 1,024 minimum is dropped. A 32,000 budget on a 32,000-output model sends 32,000 with a
  30,976 budget, never 33,024.
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
  asks first; `glob` and directory listings still show their names. Every read path, including
  @ expansion when it lands (gap item 13), goes through `Context::ask_to_read`. The shell is not
  covered: `cat .env` asks as a command, not as a secret read.
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
- Every tool result reaches the model within 64 KB (`tool::spool::MAX_RESULT_BYTES`). `read`
  pages within it by itself (whole lines, at least one per page, with the offset to continue from)
  and a directory listing shows 1,000 entries and counts the rest. Anything else past the bound
  (MCP results, skills, fetched pages, tools yet to come) is cut to its first and last 16 KB in
  `run_call`, with the whole result in `<data>/tool-output/<session>/<call>.result.log` and
  `metadata.resultFile` pointing at it.
- Background processes do not outlive the call. Once the shell exits, output still in flight gets
  500 ms; a pipe still open after that is held by a background descendant, so the call finishes
  with the shell's exit code, the descendants are stopped, and the result says so. A command with
  no time limit therefore cannot wait forever on a background process's inherited output.
- Prompt admission is one transaction (`Store::admit_prompt`). If it fails, the session's
  busy reservation is released and nothing half-written remains. `Prompt.submissionId`  is
  optional and durable: the `submission` table records id, session, message and a hash of
  the payload. Resubmitting with the same id and payload returns the original receipt, even
  after a restart; the same id with a different payload or session is a 409. The UI sends a
  fresh id with every prompt.
- Only `done` and `aborted` assistant messages are replayed to the model. `error` and
  `streaming` rows stay in the transcript as audit history and never enter a request.
- Token refresh is single-flight per provider and fenced: the first turn to notice an
  expired token refreshes it, later turns wait and reuse the stored result, and the refreshed
  pair is written only if the stored credential is still the one the refresh started from
  (`Credentials::replace_if`). Every credential mutation (`set`, `remove`, `replace_if`)
  holds the same lock, so a sign-in or sign-out during the refresh wins and is never undone.
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
- `DELETE /sessions/{id}` is the real purge (cascades messages, parts, todos, submissions).
  `PATCH { archived: true }` only archives. The UI's purge coordinator calls delete and
  reports success only when the engine confirms.

Open, deliberately: one credential slot per provider (an API key and a subscription sign-in
replace each other; account profiles are M3 work), and the shell's `session_meta` archive
table still exists beside the engine's `archived_at` until M4 folds shell tables in.

## M2 behaviour

- **MCP.** Servers live in the engine's `mcp_config` table (`PUT /mcp/{name}` with a stdio or
  http config). A saved config is approved by hash (`POST /mcp/{name}/approve`); changing
  the config withdraws approval, so a rewritten command is looked at again before it runs.
  Approved, enabled servers connect at startup and on demand through rmcp. Their tools join
  the registry as `<server>_<tool>`; tools the server marks read-only run without asking,
  the rest ask under kind `mcp` with pattern `<server>/<tool>`, and "always" therefore
  covers the whole server. Every save, disable, disconnect and remove bumps the server's
  generation; a connect that began under an older generation closes what it opened and
  publishes nothing. MCP OAuth is not implemented yet.
- **Config.** `Config::load` reads `~/.config/drift/drift.json` then `<workspace>/drift.json`
  (project rules first, so they win), plus `.drift/agents/*.md`, `.drift/commands/*.md` and
  skills from `.drift/skills`, `.agents/skills` and `.claude/skills` at both roots (project
  shadows home). `AGENTS.md` beats `CLAUDE.md`. `GET /workspaces/{id}/config` serves the
  merged result. Front matter is `key: value` lines only.
- **Agents.** `build` and `plan` are built in; `plan` gets only read-only tools and its
  prompt. A project agent of the same name replaces a built-in. A session's `agent` is set
  on create or `PATCH`; the agent's prompt is appended to the system prompt, its `tools`
  list filters the registry, its `model` is the default when the session has none. The
  filtered set is pinned for the run: a call to any tool outside it is refused before
  permission, snapshot or dispatch, so plan mode cannot write even if the model asks.
- **Commands.** `POST /sessions/{id}/command` expands `$ARGUMENTS` in the template and
  submits the result as a turn.
- **Skills** are listed in the system prompt by name and description; the `skill` tool
  returns SKILL.md's body and its directory.
- **Formatters.** After a mutating tool succeeds, the first formatter whose extensions
  match each written file runs. Built-ins (prettier, rustfmt, gofmt, ruff, black) apply
  only when on PATH; `drift.json` `formatters` can set a name to `false` or to
  `{ command, extensions }` with `$FILE`. Results land in the call's `metadata.formatted`;
  failures are ignored.
- **Permissions** resolve in order: session "always" answers, the workspace's `drift.json`
  rules, then the global policy.
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
    `> nul` counts as a write. Quoted or escaped operators are text. Redirections are listed after
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
    a path, is granted literally. Nothing widens to a bare program name.
- **Sign-in.** Anthropic offers Claude Pro/Max and Console (paste-the-code flows); OpenAI
  offers ChatGPT through the Codex flow, where the engine listens on `localhost:1455` and
  the callback route completes on its own.

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

- `tests/conformance/`: bun tests that build and spawn the real `drift-engined`, point
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
