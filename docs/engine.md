# Engine integration

Drift drives a stock `opencode serve` process. Nothing engine-side is forked or patched;
all opencode config (agents, MCP servers, plugins, providers) applies unchanged.

## Process

- Dev: `bun run dev` spawns `opencode serve --port 4096` and vite, forwarding
  `OPENCODE_SERVER_PASSWORD` to the frontend as `VITE_ENGINE_PASSWORD` (the server
  enforces basic auth whenever that env var is set).
- Shell: `src-tauri/src/main.rs` spawns `opencode serve --port 0` with the password env
  removed (localhost only), parses the printed URL, and serves it via the `engine_url`
  command. The child is killed on exit.
- The opencode CLI version must match the SDK generation. Symptom of drift: prompts 500
  with `SQLiteError: no such column ...` because an older CLI reads a newer shared DB.
  Fix with `opencode upgrade`.

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
