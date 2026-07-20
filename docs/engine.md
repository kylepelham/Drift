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

## Engine update runbook

1. `git subtree pull --prefix engine/upstream https://github.com/sst/opencode.git dev --squash`
2. `bun install --ignore-scripts` inside `engine/upstream` (skips optional native grammars).
3. `bun run build:engine` from the repo root to rebuild `src-tauri/binaries/drift-engine.exe`.
4. Restart the dev loop or the app, then confirm the new version in Settings > About
   (served live from `GET /global/health`).
5. Smoke: send a prompt, run a tool, answer a permission. Schema errors mean step 3
   was skipped or failed.

## Surface used

| Concern | Endpoint |
| --- | --- |
| Sessions | `GET/POST /session`, `PATCH/DELETE /session/{id}` |
| Transcript | `GET /session/{id}/message` |
| Prompt | `POST /session/{id}/prompt_async` (body: parts, model, agent) |
| Revert | `POST /session/{id}/revert` and `/unrevert` (message and file rollback) |
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
- Pending permissions only arrive as `permission.updated` events, and only for the
  active directory's instance. Reload the UI (or switch workspace) and they're gone
  from local state while the engine drain stays parked waiting: the run looks stuck
  with no prompt, and revert bounces off the busy session. Drift refreshes
  `GET /permission` (missing from the generated SDK, fetched raw) for every workspace
  directory on connect plus a 10s tick, and replies route to the owning instance via
  an explicit `directory` query.
- `DELETE /session/:id/share` revokes the remote share but the session record keeps a
  stale `share` property (200 response body included). Drift clears it locally after
  unshare; the stale value resurfaces on rehydration until fixed upstream.
- Solid store `set("sessions", id, info)` merges objects, it does not replace. Keys the
  engine dropped (like `share` and `revert`) must be cleared explicitly with `undefined`;
  all session upserts go through `putSession` in `src/engine/store.ts` for this reason.
  A stale `revert` marker silently hides every message sent after a revert.
