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
- [ ] MCP through rmcp: stdio, HTTP, approval, reconnect, reload
- [x] `todowrite`, `question`, `webfetch`
- [ ] `skill`
- [x] Read-only tool calls run concurrently
- [ ] Async questions
- [ ] Formatter hooks
- [ ] Config loading: `drift.json`, agents, commands, skills, instruction files

## M3: tree and lifecycle

- [ ] `task` subagents
- [ ] `spawn_thread`, `read_thread`
- [ ] Fork: bounded and active
- [ ] Move with busy guard
- [ ] Compaction with recovery
- [ ] Retry with model switch
- [ ] Revert and diff
- [ ] Shell timeout
- [ ] Per-session runtime config snapshots
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
