# Architecture

Drift is three layers with strict one-way flow:

```
opencode engine (sidecar process, HTTP + SSE)
        ^                |
   actions (REST)   events (SSE)
        |                v
src/engine  -> engine store (solid-js store, single source of engine truth)
        |
src/ui      -> components read the store, call actions. Never fetch.
src/state   -> app-level state: theme, selection, prefs (localStorage-backed)
src-tauri   -> shell: spawns the sidecar, exposes engine_url, owns Drift's SQLite (planned)
```

## Layers

- `src/engine/connection.ts` resolves where the engine lives: Tauri `engine_url` command
  when running in the shell, `VITE_ENGINE_URL` + basic-auth env vars in browser dev.
- `src/engine/sse.ts` is a minimal SSE reader over fetch. We own reconnect behaviour in
  `index.tsx` (`pump`); the SDK's built-in SSE client proved flaky so we bypass it.
- `src/engine/store.ts` holds the state shape plus pure helpers (`visibleSessions`,
  `resolveModel`, `sessionBusy`). No IO.
- `src/engine/events.ts` is the reducer: one function per event type, applied with
  `produce` for fine-grained solid updates.
- `src/engine/actions.ts` is the only place REST calls happen.
- `src/engine/index.tsx` glues it together: provider, hydration, event pump.

## Rules that keep this sane

- UI components never import from `@opencode-ai/sdk` except types via the engine layer.
- Engine layer never imports UI.
- Anything persistent and Drift-specific (workspace names, icons, archive state,
  attachments) belongs to the shell's SQLite store (phase 4.5), not the engine.
- Transcripts are only loaded for sessions the user opened (`loaded` map); events for
  unloaded sessions only touch cheap state (status, sessions list).

## Known constraints

- The engine's event stream is per-directory instance. Sessions are filtered to the
  active directory (`visibleSessions`) so the stream and the list always agree. Multi-
  workspace support means one stream per workspace (phase 4).
- The `/provider` response is richer than the SDK's stale type; `ProviderInfo` in
  store.ts models what the server actually returns (models keyed by id, with
  `capabilities.toolcall`). One cast at the hydration boundary.
