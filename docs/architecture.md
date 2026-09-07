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

Chat Markdown resolves local links from their raw `href` against the owning session's
directory, never the selected workspace or the webview URL. Windows paths, file URIs,
and relative paths open an enabled file preview when a native or companion backend is
available. Disabled or unrecognized types use `open_file_in_editor`; `#L42` and `#L42C7`
carry line and column positions. The preview's explicit Open in editor action uses the
same command. It reports an error rather than falling back to a system association,
so a document link cannot launch a script that way. Explicit HTTP(S) links remain browser
links. Only copy buttons created by Drift consume copy clicks; authored `data-*`
attributes cannot bypass link handling.

Local chat images use the same owning directory and bounded preview reader. Sanitization
removes their raw `src` and responsive `srcset` before insertion, leaving only internally
generated image markers. External HTTP(S) and data images retain transcript behavior.
`src/ui/markdown-images.ts` shares local loading with document previews, caching at most
12 unique paths within a conservative 20 MiB byte budget. Streaming replacements reuse
cached results; workspace or preference changes dispose the cache and revoke its blob URLs.
Unlinked images open the lightbox by click, Enter, or Space; explicit image links retain
navigation. Local lightbox images own a separate blob URL so transcript cleanup cannot
invalidate an open viewer. Closing or replacing the lightbox revokes that URL.

General settings persist file preview mode as All, None, or Custom, with All the default.
Custom keeps a toggle for each type; switching modes preserves those choices. Filename
classification in `src/file-preview-types.ts` allows Markdown and text/code up to 2 MiB
each, PDF and audio up to 20 MiB each, images up to 10 MiB, CSV/TSV up to 5 MiB, and video
up to 40 MiB. Text formats require valid UTF-8 without NUL bytes. Tables show at most
200 rows and 50 columns with a truncation notice. Image and media decoding depend on
webview support. Previews never write files; failures stay in the dialog rather than
automatically launching another application. Read failures offer Retry, and Open in
editor remains available even when previewing fails.

`read_file_preview` returns bounded base64 bytes through Tauri or the authenticated
companion RPC, not an engine file endpoint or a public file URL. Remote reads use the
same host/origin checks and credential-revocation handling as other companion commands.
The reader canonicalizes the supplied original workspace and target, requires a regular
file inside that root, then checks the opened handle's final path before reading to
reject symlink and path-swap escapes. Windows and Linux support this handle check; other
platforms fail closed. Metadata checks and a limit-plus-one read reject oversized files,
including growth past the limit, with a backend ceiling of 40 MiB. The frontend also
validates response size.
The workspace root comes from the request, not a separate server-side workspace grant.

Markdown documents use a separate sanitized rendering policy. Relative links and images
resolve beside the document while all preview reads retain the original workspace root.
Only enabled local images load, through the bounded reader, with at most 12 unique paths and
a conservative 20 MiB byte budget. Remote images and authored active content are blocked.
Code files render as text; standalone SVG files render only in an image element, never as
an embedded document. Media uses revocable blob URLs. Rendering does not fetch external
document resources or execute document scripts; explicit Markdown browser links remain user actions.

`.htm` and `.html` files default to a rendered page with Preview / HTML source tabs in
`src/ui/html-preview.tsx`. They retain the existing text preview preference and 2 MiB UTF-8
limit. A separate DOMPurify instance preserves document styles and inline SVG but removes
active elements, navigation, and resource attributes. A CSP placed before user styles
blocks network resources, forms, and scripts while allowing inline CSS and embedded data
images/fonts. The iframe sandbox allows only same-origin access so trusted host code can
forward Escape to the modal; it never allows scripts, forms, popups, or top navigation.
External/local companion assets and JavaScript-driven content are not loaded. The source
tab displays the original text, not the sanitized document. Other code links are unchanged.

Attachment lightboxes and image file previews share `src/ui/image-viewer.tsx`. Lightboxes
place the filename, zoom controls, and close button in one header row, hiding secondary
metadata on narrow screens. Ordinary wheel input zooms around the image pixel under the
cursor; dragging pans, and two touch
pointers pan and zoom around their moving midpoint. Fractional layout measurements keep
these coordinates accurate under CSS zoom. Images use transforms rather than oversized
scroll containers, with a numerical ceiling of 1024x actual pixels and enough overlap
retained to grab an image after panning. Reset zoom fits the image without upscaling;
1:1 shows actual size. Double-click toggles between fitted and enlarged views. Keyboard
controls are +/-, arrow keys, 0/Home to fit, and 1 for actual size. Resizing preserves
either the fitted view or the zoomed image center; changing the source resets the view.

PDF previews lazy-load PDF.js and its bundled worker, rendering one canvas page at a time
with page navigation, `#page=N`, and 25-400% zoom. Canvas output is capped at 8 million
pixels and 8192 pixels per side. There is no text selection/search layer, interactive
forms or links, document scripting, or password entry. XFA, WASM, and worker fetches are
disabled; no external CMap, font, or WASM URLs are supplied. PDFs needing those resources
may render incompletely, and password-protected files report an error.

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

General settings can opt live assistant text into a smooth burst reveal. Markdown preserves unchanged
top-level blocks, and engine updates queue behind an active reveal instead of replacing its animated
DOM. The latest appended rendered text is split into bounded inline segments and receives staggered
CSS opacity animation at the chosen typing speed. Small updates reveal per character, while large
updates group characters into at most 240 segments. Delayed segments stay out of layout until their
fade begins, so invisible backlog cannot expand the message. JavaScript never mutates text or reparses
Markdown from an animation frame. Initial, mounted buffered, and historical renders stay instant. A
live reveal drains normally after provider completion; reduced motion, loss of live status, or Stop
completes it immediately. Transcript scrolling never changes reveal progress.

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

Compaction retains complete recent turns within a 15,000-token ceiling instead of
splitting messages or keeping only two turns. Its structured update prompt carries
forward unresolved objectives, constraints, decisions, and parallel work while Drift's
overflow recovery continues from that summary without duplicating the failed request.

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
