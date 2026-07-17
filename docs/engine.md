# Engine integration

Drift embeds the opencode engine. Upstream source is vendored at `engine/upstream` via
git subtree and never edited; all opencode config (agents, MCP servers, plugins,
providers) applies unchanged. Users do not install opencode.

## Embedded engine lifecycle

- Vendoring: `git subtree pull --prefix engine/upstream opencode dev --squash` pulls a
  new engine drop. One-time setup after a pull: `bun install --ignore-scripts` inside
  `engine/upstream` (native tree-sitter grammars are optional, wasm is used at runtime).
- Building: `bun run build:engine` calls upstream's own build
  (`script/build.ts --single --skip-embed-web-ui`) and copies the result to
  `src-tauri/binaries/drift-engine[-<triple>].exe`. We never maintain our own bundling
  of their code; their build script is the contract.
- Dev: `bun run dev` spawns `drift-engine.exe serve --port 4096` (cwd = repo root) and
  vite, forwarding `OPENCODE_SERVER_PASSWORD` to the frontend as `VITE_ENGINE_PASSWORD`
  (the engine enforces basic auth whenever that env var is set).
- Shell: `src-tauri/src/main.rs` locates the sidecar (next to the app exe, or
  `src-tauri/binaries` in dev), spawns `serve --port 0` with the password env removed
  (localhost only), parses the printed URL, and serves it via `engine_url`. The child
  is killed on exit. The frontend polls `engine_url` until the sidecar is up.
- Version drift symptom (engine binary older than the shared SQLite schema): prompts
  500 with `SQLiteError: no such column ...`. Fix by rebuilding the engine binary.
- The engine shares the user's global opencode data dir (auth, config, sessions), so
  existing provider logins keep working inside Drift.

## Surface used

| Concern | Endpoint |
| --- | --- |
| Sessions | `GET/POST /session`, `PATCH/DELETE /session/{id}` |
| Transcript | `GET /session/{id}/message` |
| Prompt | `POST /session/{id}/prompt_async` (body: parts, model, agent) |
| Abort | `POST /session/{id}/abort` |
| Permissions | `POST /session/{id}/permissions/{permissionID}` (once/always/reject) |
| Models | `GET /provider` (all + connected + per-provider defaults) |
| Agents | `GET /agent` |
| Directory | `GET /path` |
| Events | `GET /event` (SSE) |

## Events reduced into the store

`message.updated`, `message.removed`, `message.part.updated`, `message.part.removed`,
`session.created/updated/deleted`, `session.status`, `session.idle`, `session.error`,
`permission.updated`, `permission.replied`, `todo.updated`. `server.connected` triggers
(re)hydration; everything else is ignored on purpose.

## Gotchas learned the hard way

- Events are scoped to the per-directory instance resolved from the request. If you
  prompt a session whose directory differs from your event stream's directory, you will
  never hear about it. Keep session list and event stream on the same directory.
- `POST .../prompt_async` returns 204 even when the run later fails; failures arrive as
  `session.error` events.
- A session's active drain keeps its original model; steering a new prompt into a busy
  session does not switch models mid-drain.
- Model defaults from models.dev include non-chat models (video/image). Always filter on
  `capabilities.toolcall` before offering or auto-picking a model.
