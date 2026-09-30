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
- [ ] Typed worker execution-mode resolver: explicit override, agent default, foreground fallback; no chat-keyword switches
- [ ] Background `task` launch receipts and bounded engine-owned jobs, independent progress from the main session
- [ ] Durable terminal results and idempotent parent completion delivery at safe provider boundaries
- [ ] Parent-idle survival, worker/session Stop and restart interruption handling without automatic effect replay
- [ ] Attributed worker permission/question waits, parent-scoped task output/stop API and generated lifecycle events
- [ ] Async-worker conformance: concurrent progress, out-of-order results, cancellation races, reconnect and retained task history
- [x] Fork: bounded (`atMessage`) and active (stable history, in-flight turn left out)
- [x] Move with busy guard (subagents move along, branches stay; retarget refuses while running)
- [x] Per-action models from Settings > Agents: title (small model default, generated in the background), compaction, handoff, subagent pins
- [x] Compaction with recovery: automatic (meter threshold, Settings off switch, stops after 3 failures), overflow compact-and-retry, `/compact`; summary plus 2-turn/15k tail, nothing deleted
- [x] Retry with model switch: `session.retry` drives the retry notice, `POST /sessions/{id}/retry` switches a waiting turn's model at once
- [ ] Revert and diff
- [ ] Shell timeout
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
