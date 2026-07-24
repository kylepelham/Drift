# Architecture

Drift is three layers with strict one-way flow:

```
engine/upstream (pristine opencode git subtree)
        | temporary engine/overlays during build/test
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
- Never edit `engine/upstream` directly. Internal adaptations that cannot use a public
  plugin/API belong in `engine/overlays`; tooling applies and reverses them atomically.
- Transcripts are only loaded for sessions the user opened (`loaded` map); events for
  unloaded sessions only touch cheap state (status, sessions list).

## Workspaces

Workspaces are directories with a stored name and icon (docs/store.md). The active
workspace drives REST calls: the client carries its directory (header on writes, query
on reads). Live events come from the engine's global stream, which covers every
instance, so busy dots, thinking indicators, and pending asks stay accurate for all
workspaces at once. Switching workspaces (`EngineProvider.setDirectory`) resets only
transcript state and rehydrates the new directory; session-keyed state persists.

The sidebar keeps workspace row geometry fixed while revealing actions, so hover never
moves the thread list. Its 192-480px width is pointer and keyboard resizable and stored
as a UI preference.

## Tool rendering

Single-file edit and write tools keep their filename and stats in the clickable summary
row. A multi-file `apply_patch` uses the engine's per-file metadata to render a header,
status, additions/deletions, and numbered syntax-highlighted diff for every changed
file. Write output renders every line immediately, then lazily highlights 160-line
chunks near its own scroll viewport; large files stay complete and cheap to open.
Assistant Markdown code blocks keep their original source beside the rendered DOM and
add a top-right icon copy control, so Shiki token markup never changes copied text.

Every visible tool renderer shares one context-action registry. Built-in edit, write,
and patch providers offer file and first-change opens; Drift plugins register through
the same API for custom or wildcard tool behavior. Positioned file opens cross one
Tauri command boundary, use a cached direct GUI executable without shell probing, and
fall back to the system association if no supported editor is installed.

Desktop zoom uses the webview's native zoom API rather than CSS `zoom`, keeping pointer
events, fixed overlays, viewport units, and anchored popovers in one coordinate system.
Browser development retains CSS zoom and converts detached context-menu coordinates
through the active scale.

Modal backdrops dismiss only when a pointer press begins on the backdrop. A text
selection or drag that starts inside a dialog remains open when released outside.

Settings persist the selected interface locale, appearance overrides, per-event system
notification and sound choices, custom sound data, and global permission auto-accept.
Built-in sounds are Vite-managed URLs from the vendored OpenCode MIT sound catalog;
audio bytes load only when played and require no asset-copy build step. Custom audio is
stored locally as a capped data URL. Drift owns its English and 17 localized catalogs;
only the selected non-English catalog loads as a cached Vite chunk. Custom CSS
persistence and application are debounced.

Transcript scrolling unsticks on the first upward movement, including inside the
bottom follow zone. Virtual row resize correction uses the measured row's real viewport
position so a tall row cannot be mistaken for a short row above the viewport.

Busy turns derive an optional topic beside `Thinking` from the first heading in streamed
reasoning text. The provider/model supplies that text; Drift recognizes the same HTML,
Markdown heading, Setext, and standalone-bold forms as OpenCode and otherwise keeps the
plain indicator. Reasoning deltas accumulate in the engine store before presentation.

Tool errors use the same clickable disclosure as successful tool rows. A preference
controls whether a newly failed tool starts expanded or collapsed; it never locks the
row in that state.

An adjacent compaction-only user boundary and its assistant summary become one
collapsible transcript row. Session-level engine errors defensively end busy activity
and render after the virtualized transcript unless the assistant message already owns
the same visible error state.

User prompts carry a hover-only footer with their agent, friendly model name, time,
copy, and revert action. Revert state comes from the engine session record; the chat
filters messages at that marker while `/undo` and `/redo` move it one user prompt at a
time. Reverted prompt text is returned to the composer for editing.

## Known constraints

- REST calls are scoped to the per-directory instance; the event stream is global.
  Sessions are grouped by their `directory` field (`sessionsFor`) so cross-instance
  events never leak into the wrong workspace list.
- The engine resolves a directory to its project root (git root). A directory that
  becomes a git repo becomes a new project; sessions created before that stay with the
  old project.
- The `/provider` response is richer than the SDK's stale type; `ProviderInfo` in
  store.ts models what the server actually returns (models keyed by id, with
  `capabilities.toolcall`). One cast at the hydration boundary.
