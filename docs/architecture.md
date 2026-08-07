# Architecture

Drift is three layers with strict one-way flow:

```
engine/upstream (pristine opencode source snapshot)
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
  attachments) belongs to the shell's SQLite store, not the engine.
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

The About section shows the app version, the live engine version, whether this copy can
update itself, a link to driftagent.dev, and credit to OpenCode. Update capability loads
once with the settings host and its row always occupies space. The jellyfish uses the
same model and shaders as driftagent.dev and imports them with three.js only on mount, as
a lazy chunk outside the startup bundle. A static logo occupies the frame until the first
WebGL frame has rendered off-DOM. Reduced motion keeps that logo. Leaving About removes
the canvas before cancelling the frame loop, disposing resources, and losing the context,
so WebView2 cannot composite a cleared canvas over the next settings section.

Transcript scrolling unsticks on the first upward movement, including inside the
bottom follow zone. Virtual row resize correction uses the measured row's real viewport
position so a tall row cannot be mistaken for a short row above the viewport. Range
selection clamps stale browser scroll offsets to the current measured transcript height,
preventing blank space when a tall row collapses.

General settings can opt live assistant text into a smooth burst reveal. Markdown still
parses the complete update immediately, preserves unchanged top-level blocks, and animates
only the changed suffix for at most one second. Initial and historical renders stay instant;
reduced motion, another update, Stop, or a user scroll completes any active reveal.

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

Human-typed prompts render with conservative Markdown: backslashes stay literal, and the
markers people paste by accident stay visible rather than becoming structure. A leading
`>` does not open a blockquote, a leading `#` does not become a heading, a line of only
`-` or `=` does not become a thematic break or Setext heading, and `*`, `_`, and `~~` do
not italicize, bold, or strike text. Deliberate constructs still render: fenced code,
inline code, pipe tables, `-` lists, and links, with URLs left unescaped so autolinks
keep working. Raw HTML and XML are escaped so pasted configuration stays literal instead
of becoming transcript DOM. Human messages above 2,000 characters or 40 lines bypass
Markdown and render as exact full-height preformatted text. Their initial virtual height
comes from the known line count and code font size, while other rows estimate wrapped text,
fenced code, and tool summaries from their content. A terminal dump is therefore never
represented as a 96px row before measurement. The transcript remains the only vertical
scroller and disables native browser anchoring because its virtualizer owns resize
compensation. Assistant output is unaffected, and machine-written user-role text keeps full
Markdown by carrying `metadata.generated` on its text part, which both spawn-thread
producers set. Compaction summaries are assistant messages and were never affected.

## Updates

The updater only runs from an installed copy, detected by an `uninstall.exe` beside the
executable. A locally built release binary lives in the cargo target directory, where an
installer can never replace it, so offering an update there produced an endless prompt
against a separately installed copy: the badge advertised the released version while the
running executable and About kept reporting the older local build. Debug builds stay
excluded as before, and About names the running kind so the two are never confused.

Installing stops the engine sidecar between download and install, waiting for it to exit
rather than only signalling it, because Windows keeps the executable locked until the
process is gone. The plugin exits this process without firing `RunEvent::Exit`, so the
sidecar would otherwise survive and hold `drift-engine.exe` locked while NSIS tried to
replace it. An install that fails, most often a declined elevation prompt, leaves the app
running, so the sidecar is started again from the parameters it was first launched with.

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
