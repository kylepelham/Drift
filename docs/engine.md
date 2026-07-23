# Engine integration

Drift embeds the opencode engine. Upstream source is vendored at `engine/upstream` via
git subtree and never edited; all opencode config (agents, MCP servers, plugins,
providers) applies unchanged. Users do not install opencode.

## Embedded engine lifecycle

- Vendoring: a `--no-tags` fetch plus `git subtree merge --squash` pulls a new engine
  drop (see the runbook below). One-time setup after a pull: `bun install
  --ignore-scripts` inside `engine/upstream` (native tree-sitter grammars are optional,
  wasm is used at runtime).
- Building: `bun run build:engine` calls upstream's own build
  (`script/build.ts --single --skip-embed-web-ui`) and copies the result to
  `src-tauri/binaries/drift-engine[-<triple>].exe`. We never maintain our own bundling
  of their code; their build script is the contract.
- Dev: `bun run dev` spawns `drift-engine.exe serve --port 4096` (cwd = repo root) and
  vite, forwarding `OPENCODE_SERVER_PASSWORD` to the frontend as `VITE_ENGINE_PASSWORD`
  (the engine enforces basic auth whenever that env var is set).
- Shell: `src-tauri/src/main.rs` locates the sidecar (next to the app exe, or
  `src-tauri/binaries` in dev), spawns `serve --port 0` with the password env removed
  (localhost only), parses the printed URL, and serves it via `engine_status`. The child
  is killed on exit. The frontend polls status until the sidecar is up, surfaces an
  early process failure with its last stderr line, and times out after 45 seconds.
- Packaging: `tauri build` produces an NSIS installer bundling the sidecar
  (`externalBin`) plus `engine/opencode` as `drift-extensions/` beside the exe
  (opencode.json, package.json, plugin sources; no node_modules, the engine installs
  plugin deps on first boot into the per-user install dir).
- Iteration: `bun run build:native` compiles without bundling; `bun run package` creates
  the installer. Release builds use incremental parallel codegen and NSIS zlib so a
  warm native build takes about 9 seconds and a packaged build about 20 seconds.
- Version drift symptom (engine binary older than the shared SQLite schema): prompts
  500 with `SQLiteError: no such column ...`. Fix by rebuilding the engine binary.
- Release builds use the canonical `opencode.db`, so existing provider logins and
  sessions remain available without duplicating transcript or event data. Before the
  first switch, Drift transactionally merges any sessions created in its channel-specific
  database; that database is retained as rollback data. Existing OpenCode project paths
  are inserted into Drift's workspace list without overwriting Drift names, icons, or
  removals. Development builds keep their channel database isolated.

## Engine update runbook

1. `git fetch --no-tags https://github.com/sst/opencode.git dev` then
   `git subtree merge --prefix engine/upstream FETCH_HEAD --squash`. Never plain
   `git subtree pull`: it follows upstream's ~1000 release tags into the local repo,
   and pushing any of them would trigger Drift's own `v*` release workflow.
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
| Prompt | `POST /session/{id}/prompt_async` (body: parts, model, agent; attachments are `file` parts with data URLs, persisted by the engine) |
| Revert | `POST /session/{id}/revert` and `/unrevert` (message and file rollback) |
| Abort | `POST /session/{id}/abort` |
| Permissions | `POST /session/{id}/permissions/{permissionID}` (once/always/reject) |
| Models | `GET /provider` (all + connected + per-provider defaults) |
| Agents | `GET /agent` |
| Directory | `GET /path` |
| File search | `GET /find/file` (fuzzy paths for composer @-mentions; mention parts use `file://` URLs + `source.text`, content read engine-side) |
| Events | `GET /global/event` (SSE, all instances; frames are `{ directory, payload }`) |
| Statuses | `GET /session/status` (per-instance map of non-idle sessions) |

## Events reduced into the store

`message.updated`, `message.removed`, `message.part.updated`, `message.part.removed`,
`session.created/updated/deleted`, `session.status`, `session.idle`, `session.error`,
`permission.updated`, `permission.replied`, `todo.updated`. `server.connected` triggers
(re)hydration; `sync` and `server.heartbeat` frames are dropped in the SSE parser;
everything else is ignored on purpose.

## Gotchas learned the hard way

- `GET /event` is scoped to the per-directory instance resolved from the request, so a
  per-directory stream goes silent for every other workspace: busy dots and thinking
  indicators froze the moment you switched. Drift streams `GET /global/event` instead
  (every instance's events wrapped as `{ directory, payload }`, plus a 10s heartbeat)
  and keeps session-keyed state (status, permissions, questions, todos) across
  directory switches; only transcripts reset and rehydrate per workspace.
- Status is event-sourced, so any gap (reconnect, missed idle) leaves a stale dot.
  Hydration reconciles from `GET /session/status`: sessions absent from the map are
  explicitly set back to idle.
- `POST .../prompt_async` returns 204 even when the run later fails; failures arrive as
  `session.error` events. Drift treats that event as terminal even if the following
  idle event is missed: it clears current activity, sets the session idle, and retains
  the error at the transcript bottom until the next prompt.
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
