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
  - [ ] OpenRouter Claude caching: contract checked (top-level or per-block `cache_control`), route not selectable until OpenRouter is a catalog provider; verify with a recorded exchange
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
- [ ] Per-session runtime config snapshots
- [ ] Async questions and MCP reconnect/reload deferred from M2
- [ ] Bedrock, Vertex, xAI, Z.ai

## M4: cutover

- [ ] `drift-migrate`: sessions, messages, parts, todos, credentials, config
- [ ] `DRIFT_*` env vars and `drift` data paths
- [ ] Delete `engine/*`, `@opencode-ai/sdk`, overlays, `build-engine.ts`, `build-extensions.ts`
- [ ] Remote gateway collapses into the engine router
- [ ] Docs rewritten for the new engine
- [ ] Perf numbers versus M0 baseline in release notes

## M5: hook seam

- [ ] `Hook` trait with serde types
- [ ] Prompt overrides as an internal hook
- [ ] MCP approval as an internal hook
