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
| Dropped | `websearch`, the model-facing `lsp` tool (diagnostics after edits come from language servers instead; see Post-edit), `execute`, `plan`, share, ACP, TUI, CLI, Jev tool routing, Copilot, Azure, Cohere, Perplexity, GitLab, Venice, Poe, Alibaba, Gateway. |
| Edit | Exact match only, with line ending normalisation on both sides. A file keeps its CRLF line endings and its UTF-8 byte order mark through `edit`, `write` and `apply_patch` (`tool::text::TextFormat`); `read` shows the text without the mark, so a match never has to include it. On a miss, return the closest region so the model can re-read cheaply; the tool text and the miss both say read's `N: ` prefix is not in the file, and a miss caused by copied prefixes says exactly that (still no fuzzy apply). `apply_patch`, offered only to the GPT and Codex models whose catalog profile asks for it, finds hunks as Codex's own `seek_sequence` does, because those models write patches that rely on it: exactly, then ignoring trailing whitespace, then surrounding whitespace, then with typographic dashes, quotes and spaces read as ASCII; the first pass that matches wins, and a miss shows the closest region as `edit` does. `apply_patch` replaces `edit` and `write` for models whose catalog profile says so. It follows the same rules: every existing file it adds over, updates, deletes or moves onto must have been read this session (so a secret needs its own read approval before it can reach a diff); every source and move destination is a separate edit ask (`Tool::asks`), any refusal refusing the call; and the whole patch is read, checked and worked out before any file changes. Only a missing file counts as absent; any other read error (denied, locked, a directory) stops preparation, and so does an update to a file that is not UTF-8 (a Windows-1252 page, say), which `edit` refuses too: decoding it loosely and writing it back would replace every such byte in the whole file. Every whole-file write the engine makes (`edit`, `write`, `apply_patch`, and undo and redo putting a file back) goes to a sibling file swapped into place (`tool::stage::replace`), so a failed write never truncates its target. Each replacement is one row in `staged_replacement` (migration 14): the destination, the staged sibling (`.<name>.drift-<8 hex>.tmp`) and the backup the swap may leave (same name, `.bak`), written in one statement before either file exists. After the swap, and at startup before any tool can run (`recover_leftovers`), the pair is settled: if the destination is missing and the backup exists, the backup is moved back first; only then are the siblings removed. The moment a swap succeeds the row is marked `swapped` (migration 15), before the backup is removed: from then on the backup is old content, so a backup that could not be removed yet (a scanner holding it) is only ever deleted later, never restored, even if the file has been deleted on purpose meanwhile. Until then it sits beside the file, so it can show in `git status`. If that move or a removal fails, both files and the row stay, and the next start tries again; rows are forgotten together in one short transaction only after their files are settled, and the store lock is never held across file I/O. A row whose paths are not exactly what the engine would name for its destination is dropped without touching any file. `write` treats only a missing file as new: a file it cannot read or decode still exists, so it must have been read first, and any other read error stops the write. On Windows an existing file is swapped with `ReplaceFileW` and no ignore flags, so its ACL and attributes carry over or the write fails; if the swap moved the original aside and could not put the new file in, it is moved back, and if even that fails the error names where the original is. A file another program holds open without delete sharing cannot be swapped: the write fails with that reason and the file is left as it was. On Unix the mode carries over; owner, group, extended attributes and POSIX ACLs are the new file's. On both, a file with other hard links is not written through them: the patched path gets a new file and the other links keep the old content. On a failure every step through the failing one is put back (a step already in its before state is left alone) and the error names any file that could not be. |
| Post-edit | Formatter hooks only: built-in table, `drift.json` can add or disable, failures logged and never surfaced to the model. A formatter that changed the file is named in the result ("the file no longer matches what you wrote; read it again"), so the next edit is not built on stale text. Then opt-in checks (`drift.json` `checks`, any linter or type checker) run once per step, their problems added to the step's last writing call. Language servers (`lsp`) report the errors each writing call left, after its formatters: rust-analyzer, typescript-language-server, pyright, gopls and clangd when on PATH, each started for a workspace on the first write to a file it handles (its process tree adopted, stopped after ten idle minutes or with the engine). The call waits up to 3 s for the touched files' errors, at most ten per file and thirty in all, appended to its result and kept in its metadata (`diagnostics`); a server that is missing, crashes, is still initializing or answers late adds nothing and holds nothing up, and one that cannot start is not tried again for five minutes. The user's `drift.json` `lsp` adds, replaces (`command`, `extensions`, optional `language`) or turns off (`false`) a server; a project's file may only turn one off, since a server starts by itself. |
| Snapshot and revert | Kept. Shell out to `git` with a shadow git dir per worktree. Snapshot before every writing tool. Revert restores a snapshot; diffs are computed between snapshots. |
| MCP | Native `rmcp` (stdio, streamable HTTP, deprecated HTTP+SSE, OAuth), 2026-07-28 stateless servers found by probing with the handshake as fallback. Reconnect and reload designed in rather than patched on; no approval step. |
| Storage | One `drift.db`, one writer, WAL, strict tables. Engine tables live beside the existing shell tables. |
| Config | `drift.json` at the project root, `.drift/{agents,commands,skills}/`, `~/.config/drift/`. Instructions from `AGENTS.md` and `CLAUDE.md`: global (`~/.config/drift/AGENTS.md`), every directory up to the repository root, and subdirectories as their files are read. `drift.json` `instructions` lists more files: a path relative to that file, absolute or `~/`, or a glob (`docs/rules/**/*.md`, walked as git lists files, at most 50 matches in name order); URLs are not fetched. Skills from `.drift/skills`, `.agents/skills` and `.claude/skills` up to the repository root, `skillPaths`, and `~/.config/drift/skills`, `~/.agents/skills`, `~/.claude/skills`. No runtime `opencode.json` fallback. |
| Identity | `DRIFT_*` env vars, `~/.local/share/drift` data dir. A one-time migrator runs on first launch. MIT attribution for opencode stays in `licenses/`. |
| Permissions | Upstream semantics (allow, deny, ask; path globs; "always" kept for the workspace; agent overrides) reimplemented once, with a single protocol. A deny rule is checked before any kept "always". |
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
already covered. A text or reasoning part still streaming is read as far as its deltas have
gone (the store keeps it in memory, in step with what was published, and writes it to disk
every 2 s and as it closes, so a crash loses at most that much), and each `part.delta` names
its `offset`, the text's length before it in UTF-16 units, so a client whose snapshot already
holds a delta skips it and one cut short mid-delta completes it. An offset beyond the cached
prefix is a gap: the client leaves the text and revision unchanged and requests reconciliation.
Reconciliation waits for an in-flight transcript reload and fetches a newer snapshot after it.
Snapshot merging preserves newer live metadata but accepts a longer compatible text prefix
and snapshot-only parts from HTTP. Part-removal tombstones prevent stale snapshots from restoring
explicitly deleted parts, even when no part was cached at removal time. A later authoritative part
update clears its tombstone; purging the session clears the tombstones with its revision records.
Deltas for a removed part are ignored, so they do not cause repeated reconciliation requests.
A resume that needs no
hydrate brings the client back online by itself (`resumed`).

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
  kept in the result, marked as not a complete answer, and the call and task fail. A reply the
  provider's safety filter ended is `refused`, said as such, and fails the same way; which of the
  two is read from the message's typed `ending`, never from its error text. Either way the call
  keeps `metadata.sessionId` and `metadata.outcome` (`replied`, `incomplete`, `refused`, `failed`,
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
  with fresh ids, into a new top-level conversation titled `<title> (fork)`. The copy goes 100
  messages per transaction, parts copied inside SQLite (only a compaction boundary is read, to
  remap it), so a long history is never loaded into memory nor holds the database for long; the
  fork stays archived, out of every list, until the last page lands, and one a crash cut short is
  purged with the other archived sessions. If a selected message is gone by the time its page is
  copied (a committed undo), the partial fork is deleted and the request fails with 409 `changed`,
  never a fork with holes in it. The selected cutoff must still exist when the copy starts.
  Every failure after creation, including the final read-record copy or publication transaction,
  attempts to delete the partial copy. If cleanup fails too, the original error is returned and
  the cleanup failure is logged; the copy stays archived for later purging.
  Which messages count is decided from message rows
  alone, without parts. Without `atMessage` it copies through the last stable message: if a turn
  is running, everything from the prompt it began at is left out (recorded when it starts, so a
  prompt steered in later does not move it). With `atMessage` it stops at that message, which must be
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
- Actions are one request each (`session::oneshot`), text only. Tools stay defined so a history
  with tool calls stays valid, but the request forbids calling them (`Request::no_tool_calls`:
  Anthropic, Bedrock and Vertex Claude `tool_choice: {type: none}`, Responses and Chat Completions
  `tool_choice: "none"`, Gemini `functionCallingConfig.mode: NONE`). A reply that still makes a
  call, or ends any way but a clean end of turn, is refused and nothing it said is used.
- Titles: the first message becomes the title at once; the title model's answer replaces it in the
  background, only while the title is still that placeholder, so a rename wins. Any failure keeps
  the placeholder.
- Signed reasoning is replayed as reasoning only to the model that produced it
  (`convert::Target`): the same catalog entry, or one that runs the same model, a mode and its base
  (`Catalog::same_model`: Claude Opus 5.5 and Claude Opus 5.5 Fast send one id). Any other model,
  an action's included, reads each finished thought as plain text in its place, as opencode sends
  it, since it cannot check the signature; a thought cut off mid-stream and a redacted one (nothing
  to read) are left out. A signed thought counts as finished even in a reply stopped later, since
  the signature arrives when the thought ends.

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
  first message: there is always something to summarise. When even the last turn is over budget
  (one prompt and hundreds of calls, the usual agent run), its newest steps are kept from a reply
  onwards (`split_turn`), as opencode does, and the rest of that turn is summarised. The turn's
  prompt then rides verbatim beside the summary in the opening user turn ("The request still being
  worked on, as the user wrote it"), so the user's words never survive only as the summary retells
  them. The request window loads that one prompt beside the tail (`Store::prompt_before`), not the
  turn between them, and a later compaction carries it into the next summary's input.
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
  - `steps` (default 200): model steps that ran tools in one turn. The last allowed step, like the
    step after `repeats` trips, is a wrap-up (`WrapUp`): tools are off for it (`no_tool_calls`) and a
    reminder asks the model to say the turn stops, summarise what it found and did, and list what
    is undone, as opencode's max-steps prompt does. Early reads do not start on that reply, and a
    call the model makes anyway (a local server may ignore `tool_choice: none`) is closed unrun.
    The reply ends `limit` (`Ending::Limit`; migration 32 swaps the `ending` column to widen its
    CHECK, keeping every stored value). A conversation then pauses with the reason; a subagent
    ends on that reply, and its parent gets the write-up as an `incomplete` result (the task
    fails, saying it reached its limit), never as a finished answer.
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
- A stored part this build cannot parse (imported by `drift-migrate`, or written by a newer Drift)
  loads as `unknown` with its stored JSON in `raw` (`Part::from_stored`), instead of failing the
  whole conversation's read. It is never sent to a model, counts nothing toward compaction, and is
  written back and copied into forks byte for byte (`Part::stored`). The UI keeps it out of the
  transcript and the composer history, carrying `raw` as `metadata.driftUnknownPart` for export.
  Stored parts are always JSON: the `callId` index reads every row with `json_extract`. A row
  that happens to parse as a native part is that part, so an importer wraps what it keeps raw (see
  "Importing opencode conversations").

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
- A finished `edit`, `write` or `apply_patch` call with no `changes` at all (only imported
  conversations have those; a native call always records, even an empty list) cannot be undone.
  Undo and redo name its files in `unrecorded`, shown in their own notice, rather than pass over
  them silently (`revert::written_without_record`).
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
  walk. Git's stat cache already makes an unchanged capture cheap, and `core.untrackedCache`
  measured no better. Within one step, a whole-tree call (a writing shell line, a writing MCP tool)
  starts from the tree the step's previous whole-tree call ended on, so a run of n such calls takes
  n + 1 captures, not 2n. That reuse is safe only because a tree capture's changes are all
  observed, never undone: anything edited in the gap lands, still observed, in the next call's
  record, where before it was recorded nowhere. A file tool's capture records undoable changes, so
  its write breaks the chain (the next command captures afresh, or the file tool's write would be
  taken as observed and its undo lost), and the chain never crosses steps.
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
- Changes merge by canonical physical path, using the same case-folded identity as writer
  reservations on Windows. Different workspace owners and relative names can therefore form one
  chronological chain. A session that writes `repo/sub/a.txt` as A -> B, moves to `repo/sub`, then
  writes the same file as B -> C undoes directly to A and redoes directly to C. Each change's
  `before` must equal the previous one's `after`; a gap marks the entire chain broken and the
  file is kept in both directions, rather than partly restoring one workspace's segment.
- The shadow repo is one per workspace, shared by every session and subagent in it, so creating it
  and every index operation (tree captures, prunes) hold a per-workspace lock.
- History belongs to the workspace, not its path: the repo is named for the workspace id
  (`Snapshots::bind`, taking over a repo kept under the old path-derived name once), and every
  recorded call stores its `owner` workspace id beside its changes. Physical paths are resolved
  from those owners' current directories. A merged chain retains separate snapshot owners for its
  first `before` and final `after` endpoints. Undo reads the original bytes from the first owner's
  repository; redo reads the final bytes from the last owner's repository. Rollback retains the
  expected endpoint's repository too, so a failed marker save can restore the previous bytes even
  when the endpoints live in different repositories. Different physical files stay separate after
  a session move. Records older than `owner` use the session's workspace; a change whose owning
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

Initial async mode is selected at launch. Foreground-to-background promotion and
adding to a running background task (opencode's `waitForPromotion` and
`background.extend`, experimental there) are planned for M5. Agent teams, arbitrary
cross-agent messaging and automatic post-crash execution resume are not required
for the first implementation. `/spawn` stays a user-only,
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

- The exit audit (end of `docs/research/opencode-exit-inventory.md`, at `016f239cc`) names, for each
  overlay and parity item, the test that carries it or the decision that dropped it; its gaps are
  M4 lines in `CHECKLIST.md`.
- `drift-migrate`: sessions, messages, parts, todos, credentials to keyring,
  `opencode.json` to `drift.json` with a report of unmapped keys.
- Model-family base prompts (done; see "Each model gets the base prompt written for its family"
  below). The Settings base-prompt override (global or per family, CHECKLIST) edits these same
  recipes.
- Rename env vars and paths. Delete `engine/*`, `@opencode-ai/sdk`, overlays, build scripts.
- Remote gateway collapses into the engine router (done): `/engine/*` is the engine's own router served in the gateway's process, the engine's token added after device sign-in, and the event socket leased to the device's credentials (`api::Lease`); device auth and TLS stay in `src-tauri` (`docs/remote.md`).
- Docs rewritten. Perf numbers against the M0 baseline published in release notes.

#### Importing opencode conversations

`crates/drift-migrate` (`import_sessions`) brings opencode's conversations into `drift.db`. The
shell runs it on a background thread (`src-tauri/src/opencode_import.rs`) at startup and again
after a workspace is added. Each run reads every `opencode*.db` in opencode's data directory,
`opencode.db` first. It never writes them, and reads each inside one read transaction, so a
running opencode does not tear a conversation. Before the conversations it adds workspace rows for
opencode projects that had sessions (`Store::import_opencode_workspaces`, moved here from the
legacy sidecar's start) and tells the UI with `workspaces-changed`.

- **Progress.** The shell emits `opencode-import` `{done, total}` for every conversation it
  finishes, and the sidebar shows "Importing from opencode" with a bar until the last is in.
  A conversation an interrupted run left half written (`imported_session.complete = 0`) is not
  counted as already here (`Store::holds_session`): the next run discards it and writes it again.
- **Order.** The most recently used conversations come first, so the sidebar fills from the top
  within seconds; subagents follow once every conversation that could have started them is in,
  and one giant old conversation (47,269 messages here) no longer holds up everything newer.
  A workspace row made from an opencode project without a name takes its folder's name, as adding
  a folder does.
- **Which conversations.** One lands in the workspace whose directory it ran in, else in the one
  holding its opencode project's repository root; directories compare without case, slash style or
  a trailing slash. A subagent goes with its parent and is listed `hidden`. A conversation with no
  workspace is counted in the report by directory and skipped, so adding that workspace imports it
  on the next run. Removed workspaces are not targets.
- **Once only, in pages.** `imported_session` (migrations 33 and 34) records every import.
  `Store::begin_import` writes the session archived (out of the list) with an unfinished record,
  `import_page` writes at most 100 messages or about 2 MB per transaction, and `finish_import`
  lists it and completes the record. A run that stops midway leaves an unfinished record; the next
  one deletes that copy and starts the conversation over, and a failure inside a run discards it at
  once (`discard_import`). A conversation already present or completely recorded is skipped, so one
  the user deletes after import never comes back. A subagent whose parent failed waits for the next
  run with it.
- **Bounded.** opencode is read 200 messages at a time and each message's parts on their own
  (`source::messages_after`, `parts`), never a whole conversation (one here holds 47,269 messages
  and 2.3 GB of parts). A part stored past 8 MB is streamed off disk through SQLite's blob reader:
  a tool call keeps its name, id, state, input (up to 64 KB) and the first 64 KB of its output, the
  rest skipped as it is read; any other part that large is read whole. While an import runs, the
  shared connection's automatic checkpoint is off and a connection of its own checkpoints after
  each page (`ImportCheckpoints`), since a checkpoint inside a commit held the connection for up to
  300 ms each time.
- **Ids.** Session ids stay, so subagent links, task results and Drift's archive records still
  join. Message and part ids are minted in this engine's form from their timestamps, in opencode's
  written order, with a short hash of the opencode id: opencode ids sort above native ones, so
  without this a new turn would sort before the history. The same row always gets the same id.
- **Mapping.** Text, reasoning, tool calls, files, compaction boundaries (pointed at the new id of
  the message they kept) and todos map to native parts. An Anthropic reply's thinking keeps its
  signature (`metadata.anthropic.signature`, `redactedData`), so continuing on the same Claude
  model sends it its own reasoning back; replay already sends a signature only to the model that
  wrote it. A tool call that never finished becomes an error with no output. opencode's per-step
  bookkeeping (`step-start`, `step-finish`, `snapshot`) is dropped: its tokens are already on the
  message and its snapshots name a shadow repository keyed by the old path. Anything else (`patch`,
  `agent`, `subtask`, synthetic nudges, a call missing its id) is kept as an unknown part inside
  `{"type":"opencode","data":"<original JSON>"}`, which no native type matches: opencode's raw
  synthetic `text` would otherwise read back as a native text part.
- **Undo.** Edits in a conversation's newest 30 messages that are also under a week old get native
  undo records (`undo::records`). opencode kept only a diff per file (whole versions only in older
  builds), so each version is rebuilt from today's file by applying the diffs backwards, newest
  edit first across every conversation imported in the run: an edit, a new file `write`, and a
  patch's adds, updates, deletes and moves. A diff must match exactly where it says (line endings
  aside); one that does not, a `write` over an existing file, or a part past 8 MB ends the rebuild
  for that call's files, so neither it nor any older call on them gets a record. Versions go into
  the workspace's undo history (`Snapshots::store_bytes`) and the record carries `owner` and `at`
  as a native one does, so undo, redo and the kept-file check work unchanged. Every other imported
  writing call is named by undo as `unrecorded`. On this machine 46 of the imported calls got
  records; the window is the user's choice, since older edits almost never still match.
- **Size.** Tool metadata no view reads is left behind: a patched file's whole before and after
  (its diff becomes the `patch` its panel draws), a read's `display` and `preview` copies, and any
  diff over 1 MB (one patch to a generated file kept 331 MB). On a 19 GB opencode database with
  1,479 matching conversations the import is 2.8 GB (4.5 GB before trimming).
- **Cost, measured** on that database (release build, Windows): the first run takes about 62 s on
  its background thread at a peak of 91 MB; a store read made every 20 ms meanwhile waited at most
  123 ms, and 3 of 2,905 waited over 50 ms. Every later start finds everything imported in 20 ms.
  Before paging, the same import peaked at 2 GB and held reads up to 4.5 s.
- **Archived.** A conversation archived in opencode or in Drift (`session_meta`) arrives archived
  as of the import, so the seven-day purge gives it a full week to be restored.
- **Settings** (`drift_migrate::import_settings`, run first each time). Each item comes in once,
  recorded in the setting `opencodeImported`, so a sign-in the user removes or a server they delete
  stays gone; the last run's report is `opencodeImportReport`, and the shell logs it. What was
  left out is also grouped by kind (`LeftOut`: sign-ins, plugins, settings, servers, failed copies)
  as names only, so the window words it in the user's language.
  - `auth.json`: an API key goes in for a provider Drift has (a local server's placeholder is
    ignored); an Anthropic, OpenAI or xAI sign-in goes in with its refresh token and expiry, since
    Drift renews those itself. Other sign-ins and unknown providers are reported. A provider already
    signed in to Drift keeps its own.
  - MCP servers, from the shell's `mcp_server` table (Drift's old manager) and then opencode's
    `mcp`: `local` becomes stdio (command split from its arguments), `remote` streamable HTTP with
    its headers and any pre-registered OAuth app. `{env:NAME}` and `{file:path}` are read at import;
    one that cannot be read refuses the server rather than save a blank secret. A server is left on
    only when it was enabled in opencode and approved in Drift's old approval step (its exact
    fingerprint, `mcp_external::fingerprint`), so nothing the user never allowed starts by itself.
    A name Drift already has, or one with characters a server name cannot hold, is reported.
  - opencode's config folder: its global `AGENTS.md`, `agents/` (or `agent/`), `commands/` (or
    `command/`) and `skills/` are copied to `~/.config/drift`, where Drift reads the user's own
    instructions, agents, commands and skills; a file already there is kept. Its `plugins/` are
    named in the report and not copied. Once copied, a file the user deletes stays deleted.
  - opencode's global config: `model` (`provider/model`), `instructions` (made absolute) and
    `permission` (`read`, `edit`, `bash`, `webfetch`, a decision or a pattern map) become
    `~/.config/drift/drift.json`, written only when it does not exist yet. Everything else
    (`tools`, `plugin`, `agent`, `provider`, ...) is named in the report; plugins are JavaScript and
    Drift runs none. Project-level opencode files are not read.
- **Queued prompts.** opencode keeps prompts it queued but never ran in `session_input`; the import
  names the conversations holding any (`Report.pending`, `source::pending_inputs`) instead of
  running them, since they were meant for a moment that has passed.
- **Summary, once.** A run that brought anything in, or left anything out, keeps a summary in the
  shell (conversations, undoable edits, queued prompts, folders waiting for a workspace with temp
  folders dropped, sign-ins, servers, copied files, `LeftOut`) and emits `opencode-import-done`.
  `opencode_import_summary` hands it out once and clears it, so the window shows it in a dialog
  (`src/ui/import-summary.tsx`) after the run and never again, rather than as a permanent panel.
- **Prompt cache.** The first message sent in an imported conversation misses the provider's cache
  (Drift's system prompt and tools differ from opencode's), a one-time write for that history;
  turns after it cache as usual.

### M5: hook seam

- `Hook` trait finalised with serde types.
- Prompt overrides implemented as an internal hook to prove the seam.
- Background task controls: a foreground task the user moves to the background keeps running under
  its owner's scope and delivers like any background task; `task_output` (or a sibling call) can add
  a follow-up to a background task still running, which it reads at its next step. Both keep the
  worker's ownership, Stop fencing and one-delivery rules.

### Trade-offs kept on purpose

- Undo restores only what Drift's own file tools wrote; changes made by shell commands (`sed -i`,
  `rm`, codegen) are named, not reverted. In return undo never overwrites an edit the user made at
  the same time.
- `edit` matches exactly (line endings aside); opencode tries nine fuzzy fallbacks. Weaker and local
  models will miss more; the closest-region message is the remedy. Measure the miss rate before
  adding any fallback.
- Delegation is one level deep; opencode makes the depth configurable (`subagent_depth`).

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
- The production file fallback and `drift-engined --file-credentials` keep authenticated encrypted
  secrets in `credentials.enc`. Windows defaults to user-scoped DPAPI. Headless hosts without a
  keychain must provide `DRIFT_CREDENTIALS_KEY`, a base64-encoded 32-byte key, for AES-256-GCM; the
  key is not saved beside the file. Missing keys disable persistence rather than falling back to
  plaintext. The file uses mode 0600 on Unix; on Windows it gets a protected DACL granting the
  current user and SYSTEM access. Only the file is restricted: the data directory around it keeps
  the access it inherits, so administrators and backup tools still reach `drift.db`. Saves use private temporary files, flush and
  atomic replacement. An existing plaintext fallback is merged into the encrypted store (which wins
  where both hold a provider), read back, and removed only after that succeeds; one that cannot be
  parsed is kept. Corrupt or wrongly keyed stores cannot be overwritten silently.
  Plain `credentials.json` storage exists only in Rust's test build. Conformance binaries use
  encrypted storage with a fixture key retained across restarts, avoiding the real keychain.
  `DRIFT_ANTHROPIC_BASE_URL` points the Anthropic adapter at a fake for recorded runs.
- Claude subscription sign-in is the PKCE flow Claude Code uses (`llm/anthropic/oauth.rs`).
  Requests made with a subscription token must look like Claude Code's:
  `llm/anthropic/claude_code.rs` adds the identity and billing system blocks, prefixes tool
  names with `mcp_` and the adapter strips the prefix from what comes back. Subscription
  turns cost nothing, so their `cost` is recorded as zero.
- An API-key Anthropic request with a thinking budget and tools sends `anthropic-beta:
  interleaved-thinking-2025-05-14`, so the model thinks again between tool calls, not only before
  the first; the subscription route already sends it, and adaptive thinking interleaves without it.
  Bedrock sends it in the body (`anthropic_beta`), Vertex as the same header; both checked against
  local stand-ins, not live accounts.
- Anthropic prompt caching uses all four breakpoints: the last tool, the system prompt, and the
  last cacheable block of the two newest user messages. The newest writes the whole prefix; the
  one before it sits exactly where the previous step wrote, so a tool loop pays only for each
  step's new blocks even past the 20-block lookback. Thinking blocks and empty text never carry
  one. The marks are set in the adapter's shared body, so key, subscription (whose extra system
  blocks add none) and Anthropic-dialect gateway base URLs all cache alike.
- A Codex (ChatGPT sign-in) request also carries `session-id` (the session, so the backend keeps a
  conversation's requests together) and, when the access token's claims bind the account to a
  region (`chatgpt_compute_residency` other than `no_constraint`), `x-openai-internal-codex-residency`,
  as upstream's Codex plugin sends them. The websocket transport is not used.
- A ChatGPT sign-in sees the catalog as the Codex backend takes it (`Engine::catalog_view`,
  `openai::codex::shape`), in the picker, planning, steering and retries alike: GPT models after
  5.4 plus the ones Codex names (`gpt-5.4`, `gpt-5.4-mini`, `gpt-5.3-codex-spark`, ...), never a
  `-pro` model or bare `gpt-5.6`, all at no per-token cost, and the 5.5 and 5.6 lines with a 400k
  window and a 272k prompt cap, so compaction runs before the backend's own limit. Signing in or
  out publishes `catalog.updated` so the picker reloads. Nothing is priced to choose a small
  model by, so titles run on `gpt-5.4-mini` when the backend offers it (`codex::small_model`),
  never an API-only model.
- One-shot requests (titles, summaries) on a reasoning model run at its weakest level with
  4096 tokens of thinking room on top of the answer's own (a budget level adds its budget),
  within the model's output limit; a budget that cannot fit is dropped.
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
  - Reasoning a model streamed as `reasoning_content` goes back to the same model, on the replies of
    the turn under way and nowhere earlier: Kimi, GLM and DeepSeek thinking models expect it there,
    and ignore or refuse it from earlier turns. The session draws that line
    (`convert::drop_earlier_reasoning`, at the prompt the turn started from), since on the wire a
    prompt steered in mid-loop and one sent after a Stop both sit beside the last tool results.
    Only finished replies give it; Anthropic never receives unsigned thinking.
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
  snapshot captures per workspace only; two sessions or workers editing one source file take turns
  through `tool::lock` (below).
- The reply's leading run of `read`, `grep` and `glob` calls starts while the reply still streams
  (`session::early`), each as soon as its block closes, provided its arguments fit the schema and
  every rule allows it without asking. The first call to any other tool ends the run, since a read
  after a write, a command or a subagent must see what that did. The step still admits each call
  as usual and takes the early result in its place; a reply that fails, is cut short or is stopped
  drops them all, and what an early read read counts as read only once its result is used (it ran
  against a copy of the session's read record). So reads overlap the rest of the reply without a
  broken reply's calls ever reaching the model.
  `edit`, `write` and `apply_patch` refuse existing files the session has not `read`; every
  mutating call records what it changed in its part's metadata. A shell line that only reads and
  exits 0 counts too for each file it printed (`cat`, `type`, `head`, `tail`, `Get-Content`, and
  `sed -n '<range>p'`, which now also counts as reading; `command::files_read`), since GPT and
  Codex models habitually read that way before `apply_patch`. A printer whose output feeds a pipe
  (`cat a.rs | grep fn`) counts nothing, the model having seen only what the pipe let through. A
  line that moves directory first counts nothing, its paths no longer resolving from the workspace. The record is kept in
  `drift.db` (`read_file`, migration 26, removed with its session), so a file read before a restart
  may still be edited after it. Each read is stamped in id order (`id::stamp`, migration 27), and a
  fork or spawned thread is given the reads made before the first message it did not copy: the
  reads its copied history shows, and no others.
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
  object fails before dispatch, and so does one whose arguments do not fit its tool's input schema
  (`tool::schema`: `type`, `required`, `properties`, `items` and `enum` are checked, other keywords
  pass, and a `null` for an optional property counts as left out); the result names every problem
  (`` `limit` must be integer, not a string``), so a wrong type is never read as missing.
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
- Gemini and Vertex Gemini send the original tool schema in `parametersJsonSchema`. Numeric enums,
  nullability and multi-type unions retain the execution contract; no string-enum conversion or
  first-type selection remains. Returned values are checked against that same schema.
- Gemini thought signatures belong to individual parts, including calls with no preceding thought.
  `PartSignature` updates the open part's `provider_signature`, persisted on `part` by migration 30
  and copied into forks. Replay wraps that block with its opaque signature only for the model that
  produced it, and Gemini places it on the original part, never on the first part or earlier thought.
  Gemini often ends a reply with an empty text part that carries only a signature; that part is
  replayed only to Gemini, since Anthropic, Bedrock and Vertex Claude refuse an empty text block.
  Other provider adapters unwrap the content without sending Gemini's signature.
- One-shot titles and compaction summaries require a normal `EndTurn` and no attempted tool calls.
  Length-limited, refused, context-exhausted, unknown and unterminated replies fail instead of
  publishing partial text. A failed summary leaves the previous valid model-visible context intact.
- A turn captures its workspace config, model/provider catalog, MCP tools and MCP instructions.
  Steering validates against that captured generation; new agents and models take effect on the
  next turn. Model-profile and agent changes rebuild the offer from the same captured tool set,
  so switching agent, model or reasoning level cannot introduce a newly connected server or its
  instructions. Retry switches resolve routes from the same catalog; credentials can refresh.
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
- **Modes (fast, ultrafast, flex, pro).** models.dev's `experimental.modes` become entries of their
  own, as opencode lists them: `claude-opus-5-5-fast` "Claude Opus 5.5 Fast", `gpt-6-astra-ultrafast`
  "GPT-6 Astra Ultrafast". Each is the base model at the mode's prices (a price the mode leaves out
  is the base's) with a `mode` (`ModelMode`): the base id sent on the wire (`Model::wire`), and the
  body fields and headers to send. Every adapter lays the fields over its own body, objects merged
  key by key (`speed: "fast"` on Claude, `service_tier: "priority"` or `"ultrafast"` on OpenAI,
  `reasoning.mode: "pro"` beside the effort), and a mode's `anthropic-beta` joins the route's own in
  one header, subscription betas included. Small jobs (titles) never pick a mode: `flex` is cheap
  because it is slow. The bundled snapshot carries the modes, and the cache file was renamed
  (`models-2.json`) so a cache an older build wrote, which dropped them, is not read.
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
    Z.ai also always gets `thinking: {type: enabled, clear_thinking: false}`, so a turn's earlier
    reasoning is kept, as opencode sends it.
- With no level picked, the catalog decides what a request still asks for (`catalog::default_reasoning`,
  `verbosity`, `shows_thinking`, `sampling`), as opencode does: an OpenAI reasoning model that
  offers `medium` runs at it (its own default), so summaries come back; GPT reasoning models other
  than Codex get `text.verbosity: low`; Gemini reasoning models get `thinkingConfig.includeThoughts`.
  Sampling the makers tune for is sent on models that take it, read from the catalog family:
  Kimi (1.0 and top_p 0.95 when thinking, else 0.6), GLM (1.0), MiniMax (1.0, top_p 0.95,
  top_k 40) and Gemini from 2.5 on (by release date) but not Lite (1.0, 0.95, 64). Chat Completions has no `top_k`, so it is
  not sent there. Claude gets none.
- Only valid completed blocks are replayed. An aborted message keeps its finished text; its
  unsigned reasoning and any call cut off mid-stream are dropped, along with the results those
  calls would have needed. A call whose arguments never parsed is answered with the parser's own
  complaint and the start of what was sent, then replayed as the same call with `{}` and that
  error, so the next request differs and the model can fix the call instead of repeating it. A
  tool named in another case (`Read`) runs as the one offered tool of that name.
- Call ids are unique within a session: `Store::add_part` gives a tool call an engine id when the
  provider sent none or one the session already used (Gemini and compatible servers number calls
  from one in every stream, Kimi repeats `functions.read:0`), looked up through an index on the
  part's call id (migration 31). Tasks, spilled output and recovery all find a call by that id. The
  Anthropic wire (Bedrock and Vertex Claude too) maps any character outside `[a-zA-Z0-9_-]` to `_`
  on both the call and its result, so history from another provider replays there. Sessions
  stored before this can hold one id twice; replay (`convert::unique_ids`) suffixes a repeat
  (`call_1_2`) on its call and its result alike, so such a session still switches to Claude.
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
    mention read in full is recorded in the session's read ledger once the prompt is admitted (a
    refused prompt showed the model nothing), so the model can edit the file straight away; one cut short is not, since the model has not seen all of it. The stored part
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
    attachments, PDFs (base64 data starting `%PDF-`) only to one whose entry says `pdf`, whole, as
    each wire's document block. Anything else (audio, video, a non-`file`/`data` URL), and an image
    or PDF for a model that cannot read it, is refused with 400 `attachment` naming the file, never
    dropped. The UI reports a model's PDF input from that same `pdf` flag, and still extracts PDF,
    text and CSV attachments to text before sending, which every model reads.
  - `GET /workspaces/{id}/files?query=` serves @ autocomplete: the same walk as `glob` (ignore
    rules apply, version-control internals never), directories with a trailing `/`, ranked by name
    prefix, name substring, path substring, then letters in order.
- `glob` and `grep` never descend into `.git`, `.hg`, `.svn` or `.jj`, and `grep` stops a
  file at its first NUL byte, so binaries produce no matches.
- `read` loads a file whole up to 10 MB. Past that (a log, generated output) it reads the page
  in 64 KB buffers from the start, on a blocking thread that checks Stop between buffers. Each
  line keeps at most 8004 bytes, even while skipping to an offset, so a newline-free file never
  grows the buffer with the file's size. Memory holds one page;
  it gives no line count, which would mean reading the whole file, and says whether more lines
  follow. Such a file can be read but not edited: undo keeps nothing over 10 MB.
- `grep` searches on several threads, as ripgrep does: it lists the first 200 matches by file then
  line and says how many there were in all, so a cut list is not just whichever files a thread
  reached first. Past 2000 matches it stops, so a broad pattern in a large tree returns at once,
  and says so: the 200 shown are then sorted from the files it reached, and earlier files may be
  missing. A Stop ends the walk and the file search in progress.
- Writers reserve canonical paths atomically across sessions and workers (`tool::lock`). File
  reservations deduplicate their complete path set. Tree reservations conflict with overlapping
  tree roots and every file beneath the root, regardless of the writer's workspace. A fixer in
  `C:/repo` therefore excludes writes to `C:/repo/sub/a.rs` from either a nested workspace or an
  approved outside-workspace write. Disjoint reservations can run concurrently; earlier conflicting
  requests take priority, and dropping a queued acquisition removes it without retaining any paths.
  File tools hold their reservation before computing the approval preview through approval,
  execution, formatting and history recording. Whole-workspace checks reserve the tree. Undo
  gathers all historical paths into one reservation, even across overlapping workspace owners,
  and holds it through marker commit or rollback. Stop can cancel undo's acquisition before any
  files change. Shell commands and external editors do not participate in these reservations.
- Checks use `platform::process::spawn_owned`. On Windows the child starts suspended, is assigned
  to its job and only then resumes. Adoption or resume failure kills and reaps the child instead
  of allowing unowned execution. Stop, timeout and normal completion terminate lingering descendants
  and wait for the whole tree to finish before history capture resumes. Windows cleanup polls the
  job's active-process count after releasing the parent handle; an exited parent is not proof that
  its descendants exited. A query failure retains the writer reservation, logs and retries rather
  than recording while writers may remain. Fixer changes are then recorded or restored before
  releasing the reservation.
- A mutating call refuses to run if its snapshot cannot be taken or its start cannot be
  recorded, and says so in its result. A result whose save fails is published as an error,
  never as a success the store lacks; a message whose terminal save fails stops the turn.
- Stopping a shell stops its descendants: a Windows job object with kill-on-close, a unix
  process group. Dropping the run future has the same effect as an explicit abort.
- The shell is `DRIFT_SHELL` when it names a file (bash, sh or zsh by name, else PowerShell);
  else Git's bash on Windows, found beside the `git` on PATH (`<root>/cmd`, `bin` or
  `mingw64/bin` lead to `<root>/bin/bash.exe`, since Git puts only `cmd` on PATH), then in Program
  Files, the per-user install (`%LOCALAPPDATA%\Programs\Git`) and Scoop (`%SCOOP%` or
  `~/scoop/apps/git/current`); else PowerShell 7 (`pwsh`), else Windows PowerShell 5.1
  (`powershell.exe`). PATH is read as it is now (`platform::process::which`), so a Git installed
  after Drift started is found. The tool text names the one it runs, and how to chain steps in it:
  `&&` in bash and PowerShell 7, `;` with an explicit `$LASTEXITCODE` check in 5.1, which has no
  `&&`.
- `edit`, `write` and `apply_patch` answer the model in one line, as opencode does ("Edited
  src/a.rs (+3 -1): 1 replacement.", "Created notes.md (40 lines).", "Patched 2 files:" then
  `A`/`M`/`D`/`R` and each file's counts), so a new 1,500-line file is not echoed back. The diff
  goes in the call's metadata for the UI: `diff` (edit and write) and `fileChanges`, one record per
  file (`filePath`, `relativePath`, `type` of add, update, delete or move, `patch`, `additions`,
  `deletions`); `files` stays the list of paths written, which formatters, checks and undo read,
  and `changes` is undo's own record of the write, merged in after the tool returns. The UI's
  adapter (`adaptToolFields`) maps native `path` (for read, edit and write only, never an MCP
  tool's argument) and `patch` to the names its rows, file actions and citations read, and
  `fileChanges` to apply_patch's per-file records, with a one-file patch's diff as its `diff`.
- `read` on a path that does not exist names up to three entries beside it whose names contain,
  or are contained in, the one asked for ("Did you mean one of these?"), as opencode does; both
  names must be three characters or more, so a file named `a` is not offered for every miss.
- Shell output is captured in bounded memory (`tool::spool`): stdout and stderr in arrival order,
  whole while under 32 KB, then only the first and last 16 KB in memory with everything (up to
  64 MB) in `<data>/tool-output/<session>/<call>.log`. The result names that file and carries
  `outputBytes` and `outputFile`. A timeout or Stop keeps what was printed; the shell tool handles
  Stop itself (`Tool::stops_itself`), so the turn awaits its result rather than dropping it, and the
  call ends `error` with `stopped` or `timedOut` in its metadata.
- PDFs travel the same way: `read` returns one (up to 10 MB, known by `%PDF-`) instead of refusing
  it, and `webfetch` returns an image or PDF URL as the file rather than as text, whatever its
  content type. `webfetch` reads a body a chunk at a time and gives up once it passes 10 MB (or at
  once when `Content-Length` says so), so a huge or endless response never fills memory. It asks
  as a browser does (user agent and `Accept-Language`), since many sites refuse unknown agents, and
  when Cloudflare answers 403 with `cf-mitigated: challenge` asks once more as Drift, which often
  passes. Its own client follows redirects only within the URL's origin (scheme, host and port,
  plus the usual move from http to https on the same host):
  the user approved that URL, so a redirect elsewhere (another host or port, or down to plain
  http) is returned as text naming the target ("fetch it to follow it") and its own call asks. A catalog model reads PDFs (`Model::pdf`) when models.dev lists `pdf` among its
  input modalities, or, without them, when it takes attachments on a route whose wire carries a
  PDF whole (Anthropic, OpenAI, Google, Vertex, Bedrock). Each adapter sends its own shape
  (Anthropic `document`, OpenAI `input_file`, Gemini `inlineData`, Chat Completions `file`); a
  model that does not read PDFs gets a line instead. Files share the ten-file and 20 MB budget.
- Images reach the model (`tool::image`). `read` returns a PNG, JPEG, GIF or WebP (up to 32 MB,
  known by its bytes) as an image rather than refusing it as binary, and an MCP result's image
  content is kept instead of becoming `[image png]`, if it is one of those four formats within
  32 MB (an SVG or BMP is named, never sent); text resources are inlined, binary ones named.
  Before it is stored, every returned image is checked against what providers accept
  (`image::normalize`, on a blocking thread): one within 2000 px a side and 5 MB of base64 passes
  unchanged; a larger one is decoded (at most 16384 px a side) and scaled to fit, then down by a
  quarter at a time, each size tried until one fits: an opaque picture as JPEG at falling quality,
  then PNG; one with transparency as PNG, then JPEG flattened onto white. webfetch still stops
  downloading at 10 MB, so its images never reach the 32 MB source limit. The
  result says so ("was scaled from 4000x3000 to 2000x1500"). An image that cannot be read,
  decoded or brought under the limit is dropped with a line saying why, as opencode does, since
  a provider rejects the whole request over one bad image. PDFs pass through. The
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
  and key). A saved definition this build cannot parse (written by a newer Drift) never takes
  the others down: it is left out of startup and tool lists and shown as a failed row with an
  empty definition (`unreadable` on its status), which the editor can save over (trust cleared)
  or the user can remove; adding a server under its name is still refused, and its switches and
  connect are refused before anything is written, the manager disabling them. Their tools join
  the registry as `<server>_<tool>` (`mcp::tool::wire_name`: any character outside
  `[A-Za-z0-9_-]` becomes `_`, a name past 60 characters is cut, and a name that had to change,
  that would spell a built-in tool's (`task` + `output`), or that another server's tool spells too
  (`a_b` + `c` and `a` + `b_c`; `mcp::tool::wire_names`) ends in a hash of the
  server and tool, so no two tools share a name and `a.b` and `a_b` stay apart, while
  `my_server_search` stays readable; 60 leaves room for the
  subscription route's `mcp_` within providers' 64, so one odd tool name cannot get every request
  refused). A name once given is kept in `drift.db` (`mcp_tool_name`, migration 28) and never
  given to another tool, even while its server is away, until the server is removed or renamed,
  which frees its names: a server that connects later and would
  clash gets the hash itself, so no tool a transcript already calls is renamed. Two new tools that
  clash with each other are both hashed. Which server a tool came from is asked of the tool (`Tool::server`), never read back
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
  (project rules first, so they win), plus agents and commands from `~/.config/drift/agents`,
  `~/.config/drift/commands`, then the workspace's `.drift/agents/*.md`, `.drift/commands/*.md`, and
  skills (`config::skill_folders`, nearest first so a nearer skill shadows a farther one of its
  name): `.drift/skills`, `.agents/skills` and `.claude/skills` in the workspace and each parent up
  to the repository root, the folders `skillPaths` lists in either drift.json (relative to the file
  or `~/`), then `~/.config/drift/skills`, `~/.agents/skills` and `~/.claude/skills`. A `SKILL.md`
  counts at any depth up to six folders down, never inside `node_modules` or `.git`. Skill URLs, which
  opencode pulls at startup, are not fetched. Instructions, general first: `~/.config/drift/AGENTS.md` (else
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
- **Agents.** `build` and `plan` are built in. A project agent of the same name replaces a
  built-in. A session's `agent` is set on create or `PATCH`; its `tools` list filters the
  registry and its `model` is the default when the session has none. The filtered set is pinned
  for the run: a call to any tool outside it is refused before permission, snapshot or dispatch.
  - A subagent's prompt goes in its system prompt. A primary agent's prompt goes, in the request
    only, on the prompt that starts each run of that agent's turns (`prompt::remind_agents`, over
    the compaction view), so plan and build send the same system prompt and tools, a switch
    between them keeps the cached prefix, and a long custom prompt is sent once per run, not once
    per message. When compaction has summarised that prompt away (a summary standing for the whole
    turn still going), the summary's opening turn carries the reminder instead, so the agent never
    loses its instructions.
  - `read_only` (front matter `read_only: true`; `plan` and `explore` built in) offers the agent
    its tools as usual but refuses, before any ask, every call that would change something
    (`Tool::stays_read_only`): a writing tool, a shell line that is not only reads
    (`command::reads_only`; `git grep -O`/`--open-files-in-pager` runs a program, so it is not
    one), an MCP tool (a server's read-only mark is its own claim: enough to skip an ask, not to
    let a read-only agent act, unless the user trusts that server: a switch in its edit sheet, on
    for a new server, written with the save (`PUT /mcp/{name}?readOnlyTrusted=`,
    `mcp_config.read_only_trusted`, migration 29); a save that leaves it out keeps what the server
    had. A call is allowed only while the connection was opened from the definition saved now, so a
    connection left over from an older definition is not trusted. MCP servers live in `drift.db`,
    which a project cannot write), a `task` to a
    subagent that is not read-only.
    So `plan` can read git history with `bash`, delegate to `explore` and load skills, and still
    cannot write even if the model asks.
- An agent's front matter may set `permissions` (a YAML flow map such as `{ bash: { "git *": allow },
  edit: deny }`, JSON, or nested lines, with `*` as any kind) and a default reasoning `variant`.
  As in opencode, the last entry that matches wins, in the order written, so `{ "*": ask, "git *":
  allow }` allows `git status` and `{ "git *": allow, "*": ask }` asks. Rules from a Settings
  override resolve the same way. Both are kept as written (`Agent::permissions`) and reversed once
  in `Config::agent_policy`, so the Settings editor shows and saves them in the order the user
  wrote. drift.json and global rules are still first-match. Its rules are checked
  before the session's grants, so an agent's deny beats an "always" answer
  (`Permissions::decide_under`), and the variant applies when the prompt names none.
  `temperature`, `top_p` and provider `options` are ignored, so agents ported from opencode run:
  each is named once in the config's `warnings` (the UI shows it as a warning notice) and the
  model's own tuned sampling is used (`catalog::sampling`). An agent with
  rules that do not parse, or with an invalid Settings override, is marked with `problem`: its own
  turns, tasks, commands and actions are refused with that reason, every other agent runs, and
  the UI names it once when the config loads. A prompt cannot switch to it, mid-turn included
  (`pickable` checks `Agent::usable`), and a broken subagent is left out of the system prompt's
  subagent list, so the model is never offered it. A broken global agent or override never stops
  other agents or workspaces.
- Settings overrides an agent with exactly what the engine applies (`AgentOverride`): `prompt`,
  `model` (`provider/model`, or empty to inherit), `steps` (its own step limit), `tools` (the
  tool names it may use), `permissions` and `variant`. The shell refuses to store any other field, naming it, and the editor
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
- **Commands.** `POST /sessions/{id}/command` (`Engine::execute_command`) expands the template
  (`Command::expand`): `$ARGUMENTS` is everything typed, `$1`, `$2`, ... (any number) one argument
  each with the highest taking the rest, and a template with neither gets the arguments appended
  rather than dropped. Arguments split as a shell splits words (`config::split_arguments`): single
  or double quotes keep spaces. A template's `` !`line` `` becomes `` `line` (its output follows) ``
  in the prompt and runs as the turn's first call (`bash`, one engine-made call per line, in the
  same message as any other), through the bash permission check like the model's own: a reading
  line runs, `cargo publish` asks, and its output (or the refusal) follows the prompt as "The /name
  command ran `line`, which returned: ...". A delegating command cannot use them (400), since they
  run in the conversation. A template's `@path` naming a workspace file or directory is attached
  as an @ mention, read in under the same rules as one the user typed.
  Command front matter may name an `agent`, a `model` (`provider/model`) and `subtask`. The
  command is validated against the running config generation, the one its turn is then admitted
  with. An action agent is refused. As in opencode, a command's agent and model apply to its own
  turn only: the turn runs as them (its reply records the agent), but they are not written to the
  session (`Pick::sticky` false) and the turn does not follow the session's choices meanwhile, so
  the next prompt runs as before. Such a command needs the session idle (409 while a turn runs),
  because steering it in would switch the running turn. A prompt the user steers into a command's
  turn is the session's, not the command's: its files are judged against the session's model and
  agent, and from the next request (the newest prompt is no longer the one the turn began at) the
  turn follows the session again, so "now edit it" is not answered by a read-only command agent. With `subtask: true`, or whenever the
  agent is a subagent (a subagent never holds the conversation, whatever `subtask` says), the turn
  opens with an engine-made `task` call (`Bootstrap`) carrying the expanded prompt and the agent,
  so the worker is a foreground task owned, stopped and recovered like any other, and its answer
  reaches the parent as text. The command's model rides in that call's metadata
  (`commandModel`), read into `Context::command_model`; `task` has no model parameter, so the
  model can never send a worker to a provider of its choosing. MCP prompts (`server:prompt`)
  still fill from the server.
- **Skills** are listed in the system prompt by name and description; the `skill` tool
  returns SKILL.md's body, its directory and up to ten of the files beside it (walked as git lists
  them), and takes optional `arguments` that fill the body as
  a command template does. Every skill is also a command (`Command::skill`) unless a command of
  that name exists. Running one opens the turn with an engine-made `skill` call that passes
  through the same permission check as the model's own, so a denied skill never reaches the model.
  Engine-made calls are stored with `engineCommand` metadata and replayed to the model as the
  user's text (`/name arguments`) followed by the result, never as a call the model made.
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
- **Project commands.** What a project brings runs only once the user has allowed it: a command a
  check or formatter names in the project's own `drift.json`, and a formatter program installed
  inside the repository (`node_modules/.bin`, found from the written file up to the repository
  root; `edit::format::project_programs`), since a cloned repository can commit one. The user's
  `~/.config/drift/drift.json`, built-in formatters run from PATH and a project's `false` need no
  say-so (as custom providers come only from the user's file, opening a cloned repository must not
  run what it names). A write such a command would run on asks, a permission card of kind
  `project-commands` listing the ones not yet answered; formatters are asked about after the call
  (`formatter prettier: C:\repo\node_modules\.bin\prettier.cmd (3f2a9c10)`) and checks at the step's
  end (`check lint: eslint $FILE`), each only for the files it runs on, and a write none of them
  covers asks nothing. Answers are per command line, and each command runs or not on its own line's
  answer: always is kept for the workspace (setting `trustedCommands:<workspace>`, the list of
  allowed lines), once allows those lines for the session, deny skips them for the session; refusing
  the project's formatter never stops its checks, nor the reverse. A program's line carries its
  package's version and a 16-byte hash (too long to match by padding a tampered package, though
  the card shows it) of the launcher, every file of the package it starts, the project's plugins
  and shared configs for it (`<name>-plugin*` and `<name>-config*`, scoped or not, in the
  `package.json` beside `node_modules`, since they load at run time) and every file of what those
  depend on, peers included (`edit::package::fingerprint`: the package is found where a symlink
  points or a shim names, `%dp0%`, `$basedir` or bun's `.bunx`, else `node_modules/<name>`;
  dependencies as Node resolves them). So a program replaced at the same path, upgraded behind an
  unchanged npm launcher, or edited in `node_modules` with its `package.json` left alone asks
  again. Each write fingerprints each program once, on a blocking thread, and the formatter then
  runs only a program whose line was just allowed; file hashes are reused while size and modified
  time hold, and each fingerprint replaces its launcher's cached set, so the cache never outgrows
  the packages in use. A plugin loaded by path, or the formatter's own config file run as code
  (`prettier.config.js`), is not covered. A refused project copy is skipped, never
  replaced by one on PATH, which may be another version. Built-in formatters from PATH and the
  user's own commands always run. Subagents take the answer of the session that delegated
  to them, along the same lineage as permission approvals, so a delegated task does not ask again.
  A permission rule of that kind (pattern `*`) allows them without asking.
- **Permissions** resolve in order: a deny rule (the workspace's `drift.json`, then the global
  policy), then "always" answers, then the other rules, then the operation's default. A deny comes
  first because "always" is kept for the workspace with no end: a `git push*` deny added after an
  "always" for `git push` still holds.
  - Policy evaluation is separate from an approval dialog. Ordinary workspace reads, scratch
    access, searches, skills, delegation and read-only MCP calls carry an allow-by-default policy
    request. Explicit deny/ask rules still apply; an unmatched default request produces no dialog.
  - Workspace edits (edit, write, apply_patch) allow by default too, since undo can put them back,
    except for files that would widen what the agent may do or hold secrets: `drift.json` anywhere,
    anything under `.drift/`, version-control internals (`.git`, `.hg`, `.svn`, `.jj`) and files
    likely to hold secrets (`tool::guarded`). Writes outside the workspace still ask.
  - A shell line runs without asking when it only reads (`command::reads_only`), no move out of
    the workspace is left in it, it uses no content searcher over a directory (`grep`, `rg`,
    `git grep`, `Select-String`, which would read `.env` too; the `grep` tool skips such files),
    and every word stays inside the workspace and names no secret file: no glob, variable or `~`,
    no path resolving outside, with `--flag=value` and `rev:path` judged by their path parts
    (`bash::reads_inside`). Any word that names something on disk is judged where it resolves, so
    `cat notes`, with `notes` a link out of the workspace, asks. So `git status`, `git log`, `ls src` and `cat README.md` run, while
    `cat .env`, `ls ..`, `git show HEAD:.env` and `cargo test` ask. A rule still decides first.
  - Rules for every workspace are kept in Settings > Permissions: an ordered list (kind, glob,
    allow/ask/deny) in the `permissionRules` setting, loaded into the engine's global policy at
    startup and replaced whole by `PUT /permission-rules` (`GET` reads it). They are checked after
    the rules in drift.json and the first match wins, so a project's file still decides first. A
    rule whose kind names no operation or whose pattern is no glob is refused with the reason, and
    the list holds at most 200. The same section lists the active workspace's "always" grants with
    Revoke and Revoke all, through the routes below.
  - "Always" holds for the workspace, in every session and across restarts (`Permissions::bind`
    ties each planned session to its workspace; grants are kept in the `permissionGrants:<id>`
    setting), as opencode keeps it for the project. A session with no workspace keeps its grants
    in memory. Answering "always" also answers "once" for every other waiting ask that the new
    grant now covers, under the policy each was asked under. The card's button reads "Always
    allow in this workspace". `GET /workspaces/{id}/permission-grants` lists them (each tagged
    `grant`: `exact`, `subcommand` or `pattern`); `POST .../permission-grants/revoke` with one as
    listed takes it back (404 if it is not held), `DELETE .../permission-grants` takes back all,
    and the stored list is rewritten each time. All three answer 404 for a workspace the engine
    does not have, before anything is loaded for it. When the shell forgets a removed workspace
    (`store_forget_workspace`, only for a workspace marked removed), it first calls
    `Engine::forget_workspace`, which drops the workspace's stored and cached grants and its
    trusted project commands, then deletes its own row, so a failure in between can be retried;
    the shell never names those keys. That runs from the seven-day purge of removed workspaces,
    once `POST /workspaces/{id}/purge` has deleted their conversations.
    Searches also evaluate their `grep`/`glob` rules. An approved search covers the files under its
    path, outside the workspace and in the scratch directory too, unless an explicit rule says
    otherwise: a file a rule denies is skipped, and one a rule asks about is skipped unless the
    session already granted it, so approving a directory search cannot bypass file restrictions.
    The rules are compiled once per search (`permission::Compiled`), not per file.
  - File asks (read, edit, write, apply_patch) carry the absolute path and, inside the workspace,
    the relative one (`Ask::path`); rules and approvals match either, so a committed `src/**`
    or `src/generated/**` rule works on every machine.
  - A subagent inherits its parent's approvals (`Permissions::inherit`, registered when `task`
    creates it); it shares the workspace, so it has the workspace's grants as well.
  - Replies are `once`, `always`, `deny` and `stop`, with an optional `message`. `deny` refuses
    the call and the turn goes on; the model's result reads "The user denied permission for this
    call. They said: ..." when there is a message. `stop` refuses it and ends the turn that asked,
    as Stop does for that turn only: for a subagent's request that is the subagent (its parent sees
    it stopped and goes on), never the parent or other workers. A call a rule refuses says so
    ("A permission rule forbids this call."), never that the user did. The permission card offers
    Allow, Always, Deny and Deny and stop, with a note field whose text goes as `message` on
    either refusal.
  - A file tool's ask carries `diff`, the change it would make (`edit` worked out as the edit
    would, against the file as it is now; `write` from what the file holds or from nothing;
    `apply_patch` the patch itself), cut at 64 KB on a line. The card shows it, so the user
    approves the change, not just a path. The call holds the file locks while preparing the preview,
    waiting for approval and writing, so another session cannot add matches during that wait.
    External editors and shell commands do not participate in those locks.
    Rules and approvals never look at it. An edit that
    would not apply carries none; running it says why.
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
  - `bash` takes `workdir`, a directory inside the workspace to run in (one outside is refused,
    pointing at `cd`, which asks). Moves in the line are followed from it, and files a reading line
    prints are found from it.
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

Medians of five runs on the development machine (`bun run bench:engine`, release build, a stub
OpenAI-compatible provider that answers at once, so only engine time is measured). The opencode
numbers were recorded at M0 and are the target the native engine had to beat; the M4 column is the
native engine at cutover, with a user provider and the default family's base prompt.

| Measure | opencode 1.18.33 (M0) | native (M0) | native (M4) |
|---|---|---|---|
| Cold start, process spawn to first event frame | 1012 ms | 26 ms | 32 ms |
| Prompt accepted to provider request sent | 1066 ms | | 4 ms |
| Provider response to text event delivered | 49 ms | | under 1 ms |
| System prompt per turn | 12,089 chars | | 3,549 chars |
| Tool schemas per turn | 25,369 chars | | 14,705 chars |
| Approximate tokens per turn (chars / 4) | 9,365 | | 4,564 |

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
- The three features that read OpenCode's database now read `drift.db`:
  - The removed-workspace purge: seven days after a workspace is removed, the UI's sweep calls
    `POST /workspaces/{id}/purge` (`Engine::purge_removed_workspace`), which deletes every
    conversation of the workspace in one write (archived ones and subagents too), their shell
    output and the workspace's undo history, and announces each deletion. It refuses with 409
    while the workspace is on the sidebar again (`in_use`) or one of its conversations runs
    (`busy`), so the sweep keeps the record and tries again; 404 means nothing is left. Only then
    does the shell forget the workspace (`store_forget_workspace`).
  - Transcript search (`session_search.rs`) reads `drift.db` on a read-only connection of its own,
    so a scan never holds the engine's writer: the 500 newest conversations (spawned threads
    included, subagents not) of workspaces on the sidebar, through the message and part indexes,
    matching only text and reasoning.
  - Settings > Storage (`storage.rs`) sizes the database (transcripts and images, sampled) and the
    engine's undo history and shell output folders, and counts conversations. The opencode screen's
    event-log rules have no native counterpart: the engine already drops unreferenced undo
    history, week-old shell output and unreferenced images every six hours (`Engine::clean_up`),
    so "Clean up now" runs that same housekeeping at once and reports what it freed. Compact runs
    `VACUUM` on its own connection, refused while any conversation runs, and folds the log back
    so the file shrinks. The screen's strings stay English for now, like the rest of it.
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
- Each model gets the base prompt written for its family (`Model::prompt`, decided in
  `llm::catalog` from the tool profile and models.dev's `family`, never the id): `codex` (exactly
  the models that edit with `apply_patch`: keep going to the end, a sentence before a group of
  calls, edit only with `apply_patch`), `claude` (do what was asked and no more, prefer editing to
  new files, no speculative handling, a todo list for long work), `gemini` (confirm before
  assuming, small verified steps, no narrated tool calls) and `default`, the one prompt every model
  had before. All are Drift's own text in `session/prompts/`. `shared.txt` follows each, whatever
  replaces the family's part: it tells the model not to revert changes it did not make in a dirty
  worktree, what `<system-reminder>` blocks are, and how to shape a final answer. The agent's
  prompt follows as before. The environment section
  gives the working directory, whether it is a git repository, platform, date and the model's
  catalog name, and the scratch directory (`tool::scratch_dir`: `Drift` in the system temp
  directory, made when the engine opens), where reading, writing, editing and patching ask
  nothing (secret files still do), so temporary files stay out of the workspace. `task`'s text
  asks the model to say whether a subagent should change code or only report, how to check its
  work, and not to redo work it has handed off. A prompt sent to a writing agent right after a
  read-only agent replied carries a reminder, in the request only, that the read-only limits no
  longer apply (`prompt::remind_agents`); it stays on that prompt in later requests, so the
  prefix is unchanged, and later prompts follow a reply by the new agent, so they get none. Each MCP server whose tools the turn offers adds its initialize `instructions`
  under "# Instructions from the <name> MCP server" (`prompt::Setting`). Settings > Prompts
  edits the base prompts in the engine (`GET /prompts`, `PUT` and `DELETE /prompts/{id}`, one
  setting `basePrompt:{id}` each): "All models" (`all`) replaces every family's text, and a
  family's own replacement wins over it (`prompt::base_for`). A replacement is never empty (reset
  instead) and holds at most 64 KB. The shared rules follow it and are shown read-only. A
  conversation picks up a change when it next builds its system prompt, at its next turn. The
  shell's old `family:*` overrides, written for the opencode plugins, are not read.
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
