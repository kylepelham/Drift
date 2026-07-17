# Architecture

Drift is three layers with strict one-way flow:

```
engine/upstream (vendored opencode source, git subtree, never edited)
        | bun run build:engine
drift-engine.exe (embedded sidecar, HTTP + SSE)
        ^                |
   actions (REST)   events (SSE)
        |                v
src/engine  -> engine store (solid-js store, single source of engine truth)
        |
src/ui      -> components read the store, call actions. Never fetch.
src/state   -> app-level state: theme, selection, prefs, workspaces (Drift store-backed)
src-tauri   -> shell: spawns the sidecar, exposes engine_url, owns Drift's SQLite (docs/store.md)
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

## Workspaces

Workspaces are directories with a stored name and icon (docs/store.md). The active
workspace drives everything engine-side: the client carries its directory (header on
writes, query on reads) and the SSE stream subscribes with `?directory=`. Switching
workspaces aborts the pump, resets engine state, and reconnects scoped to the new
directory (`EngineProvider.setDirectory`). Session lists for inactive workspaces are
fetched over REST so the sidebar can group threads per workspace; only the active
workspace streams live events.

## Known constraints

- The engine's event stream is per-directory instance. Sessions are grouped by their
  `directory` field (`sessionsFor`) so the stream and the lists always agree.
- The engine resolves a directory to its project root (git root). A directory that
  becomes a git repo becomes a new project; sessions created before that stay with the
  old project.
- The `/provider` response is richer than the SDK's stale type; `ProviderInfo` in
  store.ts models what the server actually returns (models keyed by id, with
  `capabilities.toolcall`). One cast at the hydration boundary.
