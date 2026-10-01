# Checklist

Current state of the engine rewrite on `next/1.4.0-engine`. The plan and every
decision behind it are in `docs/engine-rewrite.md`. Tick items here as they land;
change the plan there when a decision changes.

## M0: skeleton

- [x] Cargo workspace with `crates/drift-engine`, linked into `src-tauri`
- [x] axum on loopback inside the Tauri process, `/health`, `/openapi.json`
- [x] WS hub: `seq`, ring buffer, `cursor` replay, `resync`
- [x] SQLite store with versioned migrations in `drift.db`
- [x] Generated TypeScript client wired into `src/engine/`
- [x] Perf baselines recorded against the opencode engine

## M1: vertical slice

- [x] Anthropic adapter: API key, subscription OAuth, streaming, tool calls, thinking
- [x] Turn loop
- [x] `read`, `edit`, `write`, `bash`, `glob`, `grep`
- [x] Exact-match edit with closest-region miss reporting
- [x] Permissions end to end
- [x] Snapshot before writing tools
- [x] Persistence
- [x] One `drift.db` connection shared by engine and shell
- [x] Real turns against Anthropic through `drift-engined` (read, edit with ask, bash with ask)
- [x] Drift UI against the new API (adapter over the legacy store shapes; engine-side verified, shell run pending)
- [x] Failure paths: terminal validation, atomic admission, replay eligibility, single-flight refresh, hydrate cursor safety, full listings
- [x] M1 sign-off corrections: resolved paths, refused unsafe writes, process-tree stop, replay eligibility, auth fence
- [x] Conformance suite with recorded Anthropic fixtures (`tests/conformance`)

## M2: breadth

- [x] OpenAI Responses, Codex OAuth, `apply_patch` profile
- [x] Gemini (adapter only; not yet exercised against the live API)
- [x] OpenAI-compatible generic with xAI, Z.ai, OpenRouter, LM Studio and Ollama presets
- [x] MCP through rmcp: stdio, HTTP, approval (reconnect and reload: M3)
- [x] `todowrite`, `question`, `webfetch`
- [x] `skill`
- [x] Read-only tool calls run concurrently
- [x] Questions (blocking; async variant is M3)
- [x] Formatter hooks
- [x] Config loading: `drift.json`, agents, commands, skills, instruction files

## M3: tree and lifecycle

- [x] Foreground `task` subagents (abort cascades, no nested delegation, sidebar only while active)
- [x] Branches: `/spawn` drafts a handoff, user reviews, engine creates an independent conversation with its cutoff; the model cannot branch; `read_thread` for branches
- [x] Typed worker execution-mode resolver: explicit override, agent default, foreground fallback; no chat-keyword switches
- [x] Background `task` launch receipts and bounded engine-owned jobs, independent progress from the main session
- [x] Durable terminal results and idempotent parent completion delivery at safe provider boundaries
- [x] Parent-idle survival, worker/session Stop and restart interruption handling without automatic effect replay
- [x] Attributed worker permission/question waits, parent-scoped task output/stop API and generated lifecycle events
- [x] Async-worker conformance: engine-level gates (concurrent progress, out-of-order results, bounded slots, Stop while idle, single-worker stop, attributed asks, restart) and HTTP/WS conformance against the fake provider (progress events, parent carries on, single delivery, cursor replay, idle Stop wakes nothing)
- [x] UI task views: engine store `tasks` from `GET /sessions/{id}/tasks` and `task.updated`; transcript task rows follow the worker, not the launch receipt; Background tasks dock with state, current tool, Stop and open
- Agent-loop review at b93cbc9 (`docs/research/agent-loop-review-b93cbc9.md`):
  - [x] Undo/redo keep a path whose change chain another edit broke
  - [x] `apply_patch`: read-before-write for every existing target and move destination, an ask per path, whole patch checked before any write, rollback on a failed write
  - [x] SSE decoding keeps characters split across reads whole
  - [x] A worker cut off at its output limit is `incomplete`, not `replied`
  - [x] Malformed data URLs refused at admission; full mentions count as reads; mention reads bounded; steering judged against the running model
  - [x] Error and token bodies read within size and time bounds
  - [x] A session reads its own spilled output without asking
  - [x] Runners and installers approved only exactly; deny rules see past assignments and PowerShell aliases
  - [x] Undo history owned by workspace id; moves keep it; pruning parses outside the database lock
  - [x] Route-configurable timeouts, local routes 600 s; Deny and stop scope stated; OpenAI `prompt_cache_key`; `require_git(false)` for the large-file walk; stale concurrency doc fixed
- Worker review at 06a5e0c (`docs/research/worker-review-06a5e0c.md`):
  - [x] Each worker has its own token, registered before planning or queueing; Stop reaches it queued, starting, planning and running; checked again after a slot is granted
  - [x] Stop and prompt admission share one fence; deliveries carry the launch-generation scope through every wait into it
  - [x] Durable per-owner Stop count and per-task launch generation; restart recovery suppresses results launched before a Stop
  - [x] One claim-and-acknowledge delivery path: `delivered` set only in the write that saves the prompt or call result; task_output and foreground included; foreground recovery writes into its own call
  - [x] Worker plan (prompt, tools, limits, workspace, model) fixed at admission; only credentials refreshed at start
  - [x] Task and child session created or reused in one transaction; replays return the mode's own result
  - [x] `apply_patch` writes through staged copies, restores the failing step too, names files it could not restore; only NotFound is absence
- Worker follow-ups:
  - [x] Owed deliveries retried on the parent's job end and on repairs (credentials, session model/agent, agent overrides), with the reason exposed; a trigger racing a held claim is kept by the attempt; no timer loop
  - [x] Results Stop suppresses are held, not delivered, and ride along with the next user prompt, once
  - [x] `task_output` is background-only; launching calls own foreground (and failed-launch) results through their save
  - [x] Staged writes: Windows `ReplaceFileW` keeps the ACL or fails; delete-sharing refusal leaves the file; hard links documented; only recorded engine-named staging files cleaned
  - [x] New comments one line each
  - [x] Startup recovery settles recorded replacement pairs: a stranded backup is moved back, a failed restore keeps both files and the record; pairs recorded in one statement, forgotten in one short transaction after file I/O
  - [x] `edit`, `write` and undo/redo go through the staged writer; `write` treats only NotFound as a new file
  - [x] Held results ride along with a later permitted delivery, in the same write
  - [x] Undo and redo are all or nothing: a failed write puts back the files already changed, so a retry reports nothing as kept
  - [x] A replacement is marked `swapped` once its swap succeeds, so a stale backup is removed, never restored over a deliberate deletion
- [x] Fork: bounded (`atMessage`) and active (stable history, in-flight turn left out)
- [x] Move with busy guard (subagents move along, branches stay; retarget refuses while running)
- [x] Per-action models from Settings > Agents: title (small model default, generated in the background), compaction, handoff, subagent pins
- [x] Compaction with recovery: automatic (meter threshold, Settings off switch, stops after 3 failures), overflow compact-and-retry, `/compact`; summary plus 2-turn/15k tail, nothing deleted
- [x] Retry with model switch: `session.retry` drives the retry notice, `POST /sessions/{id}/retry` switches a waiting turn's model at once
- [x] Revert (undo/redo with files, across subagents; the next prompt commits it). Diff: per-call diffs in tool metadata; no session diff endpoint until something consumes one
- [x] Shell timeout: Settings value pushed to the engine, model `timeout` wins, badge metadata while running and on expiry
- Native agent loop gaps (before async workers, in order):
  - [x] Undo restores only attributable paths and keeps later edits
  - [x] Snapshots: per-workspace lock, 10MB limit, retention and prune
  - [x] Shell-aware approvals: per-command decisions, no widening to the program name
  - [x] Secret files ask to be read inside the workspace, grep withholds them, examples exempt; `.git` and binaries skipped by search
  - [x] Anthropic conversation cache breakpoints (API key, subscription, Anthropic-dialect gateways)
  - [x] OpenRouter Claude caching: OpenRouter is a catalog provider; Claude models get per-block `cache_control` (system and the last two user messages) and cache usage is read; verified with a recorded exchange against a local stand-in
  - [x] SSE error classification and bounded, cancellable, `retry-after`-aware backoff
  - Review of items 1 to 6:
    - [x] Byte-exact shadow snapshots (attributes overridden, existing repos migrated)
    - [x] Changes seen during a command are unattributed and never undone
    - [x] Oversized tracked files leave the shadow index; reported as unrecorded, not deleted
    - [x] Snapshot cost measured; no tree reuse without a reliable unchanged signal
    - [x] File-writing redirections need exact approval (bash and PowerShell)
    - [x] Permanent quota errors never retry; oversized `retry-after` cannot panic; failed jobs release the session; cap after jitter
    - [x] Bounded shell capture; inherited pipes and background descendants
    - [x] `.envrc`, `cd` chains, opaque-construct docs, periodic prune (OpenRouter caching checked, left open above)
  - [x] Shared HTTP client with connect, header and stream-idle timeouts; Stop cancels while waiting
  - [x] Thinking budget within the output limit; max-tokens endings surfaced; unexecuted calls settled
  - [x] Configurable step limits and repeated-call intervention
  - [x] Shared tool-output limits with full-output artifacts (read, list, MCP, shell)
  - [x] Engine-owned steering and queueing at safe boundaries
  - [x] File discovery and @ expansion, every read through `tool::read_ask`
  - [x] Attachments validated against model capabilities; no silent drops
  - [x] Explicit denial and stop feedback; parent-to-worker permission inheritance
- [x] Per-session runtime config snapshots: a turn's `Plan` holds its config and the tool objects it offered (MCP clients included) until it ends; call-time config reads use the snapshot
- [x] MCP reconnect and reload: watched connections reconnect with capped backoff under the server's generation, planning waits up to 2 s for connects under way, an approved save reconnects at once while running turns keep their client
- [x] MCP lifecycle coordination: one lock over row writes, generation bumps, connect starts and publishes; bounded `initialize` and `tools/list`; superseded or abandoned attempts cancelled with their process tree; backoff kept until a connection proves stable
- [x] MCP tools in running turns: per-server slot so captured tools follow a reconnect of the same definition, disable and remove end calls and refuse captured tools, read-only calls retried once after a lost connection, calls that may have changed something never replayed
- [x] Async questions: default async, answers saved as a `clarification` prompt through normal admission (joins, starts, or only saves after Stop), card closes after the save, idempotent resend, engine-only parts refused from clients; per-request decision lock, submission replay settled inside the admission write, store-backed resend check that survives a restart, `question.result` frames for socket replies
- [x] Reasoning level (missed at M1): picker variants from models.dev `reasoning_options` (as opencode), mapped per wire after checking current provider docs (Claude adaptive effort or budget, Gemini level or budget, OpenAI effort, OpenRouter object, `reasoning_effort` elsewhere); variant saved per session so engine-started turns keep it (worker deliveries, question answers, subagents, branch seeds, retry with a model switch)
- [x] Bedrock (SigV4 or Bedrock API key, env and profile credentials, event-stream replies, Claude only) and Vertex (service account or ADC token, Claude and Gemini publishers); xAI and Z.ai as compatible presets. Verified against local stand-ins, not live accounts

### UI completion audit (`docs/research/ui-completion-audit-bc2a8cd.md`)

- [x] Composer agent selection runs the session as that agent (prompt `agent`, validated, saved at admission); sessions and messages keep the agent they ran as
- [x] MCP manager, registry installs and approval prompts on the native `/mcp` authority only; editor limited to fields the engine runs
- [x] Agent Settings apply what they show: overrides carry prompt, model, steps and tools (applied natively), any other field refused; editor projects the native agent
- [x] Model-family prompt editor states it is not applied and is read-only (no native family prompts)
- [ ] Native base-prompt override (global or per model family) so the Prompts editor can save again
- [x] Archive and restore (sidebar, `/archive`, Archive dialog) archive in the engine first, with its Stop, then update the shell record
- [x] Bounded fork reachable (Fork from here on a finished reply); a resent prompt keeps its submission id until the engine answers for sure
- [x] Jev tool routing removed from Settings (toggle, state module and status polling)
- [x] Review fixes: MCP secrets never leave the engine (names only, write-only values), rename and add refuse a taken name, the archive purge deletes only what the engine still has archived, a prompt for another agent or level gets its own turn
- [x] Review follow-ups: captured MCP tools refused on a redefined reconnect, approval tied to the reviewed hash, unnamed and cleared variants hash apart, the composer shows the session's saved agent and level, legacy agent overrides save again

#### Retained features still on the OpenCode database (pending native UI work)

- [ ] Removed-workspace purge: `actions.removeAllSessions` is a stub that reports nothing deleted, so the seven-day cleanup of a removed workspace never completes (its tombstone and sessions stay)
- [ ] Transcript search (`session_search`) reads OpenCode's database and schema, so native transcripts are never matched
- [ ] Settings > Storage (stats, analyze, prune, compact) reads and prunes OpenCode's database, not `drift.db`

## M4: cutover

- [ ] Every legacy capability in `docs/research/opencode-exit-inventory.md` checked off or explicitly dropped
- [ ] `drift-migrate`: sessions, messages, parts, todos, credentials, config, MCP servers from the shell's `mcp_server` table (unapproved)
- [ ] Delete the shell MCP runtime and Jev routing (`mcp.rs`, `mcp_external.rs`, `tool_routing.rs`, their commands, remote gateway entries and watcher hook); the UI no longer calls any of them
- [ ] i18n sweep: drop keys the native UI no longer uses from every locale (`drift.mcp.description`, `drift.mcp.authenticate`, `drift.settings.prompts.astraDescription`)
- [ ] `DRIFT_*` env vars and `drift` data paths
- [ ] Delete `engine/*`, `@opencode-ai/sdk`, overlays, `build-engine.ts`, `build-extensions.ts`
- [ ] Remote gateway collapses into the engine router
- [ ] Docs rewritten for the new engine
- [ ] Perf numbers versus M0 baseline in release notes

## M5: hook seam

- [ ] `Hook` trait with serde types
- [ ] Prompt overrides as an internal hook
- [ ] MCP approval as an internal hook
