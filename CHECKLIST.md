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

- [ ] Anthropic adapter: API key, subscription OAuth, streaming, tool calls, thinking
- [ ] Turn loop
- [ ] `read`, `edit`, `write`, `bash`, `glob`, `grep`
- [ ] Exact-match edit with closest-region miss reporting
- [ ] Permissions end to end
- [ ] Snapshot before writing tools
- [ ] Persistence
- [ ] Drift UI against the new API
- [ ] Conformance suite with recorded Anthropic fixtures

## M2: breadth

- [ ] OpenAI Responses, Codex OAuth, `apply_patch` profile
- [ ] Gemini
- [ ] OpenAI-compatible generic with OpenRouter, LM Studio and Ollama presets
- [ ] MCP through rmcp: stdio, HTTP, approval, reconnect, reload
- [ ] `todowrite`, `skill`, `question`, `webfetch`
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
