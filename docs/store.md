# Drift store

Everything lives in one SQLite file, `%APPDATA%/dev.drift.app/drift.db`. The engine's tables
(conversations, workspaces, MCP servers, settings) sit beside the shell's; this page covers the
shell's, which hold what the app adds on top.

## Performance posture

- WAL journal, `synchronous=NORMAL`, 128MB mmap.
- One writer and four read-only readers (`store/connections.rs`). `Store::lock` is the writer, held
  for a whole unit of work; `Store::read` takes a free reader for work that only reads. WAL lets a
  reader see every committed write while the writer works, so loading a long transcript never
  holds up a turn's writes, and reads never queue behind each other. An in-memory store (unit
  tests) has no readers and reads on the writer.
- Waiting for a connection, and reading a transcript, runs as blocking work (`store::blocking`,
  tokio's `block_in_place` on a multi-threaded runtime), so a busy store never stalls the async
  workers serving HTTP, events and other turns.
- The shell's Tauri commands that touch the store or the disk are `#[tauri::command(async)]`: none
  runs on the main thread, whose stalls Windows reports as "Not Responding".
- All statements go through `prepare_cached`.
- STRICT tables, indexes only where reads demand them (`session_meta.workspace_id`). Lookups into
  `part` by a field inside its JSON use partial expression indexes that name the same condition as
  the query: `idx_part_call` (a call by id), `idx_part_image` (a prompt numbering its pasted images)
  and `idx_part_changes` (snapshot pruning, read from the index alone). `idx_message_streaming`
  holds only replies still streaming, for startup. Tests pin each plan, so no statement scans
  `part` or `message`.

## Schema

| Table | Columns | Purpose |
| --- | --- | --- |
| `workspace` | id, path (unique), name, icon, last_used, removed_at | workspaces = directories with display identity; `icon` is empty (initials rendered from name) or a small data-URL image thumbnail |
| `session_meta` | session_id, workspace_id, archived_at | Drift metadata about engine sessions; today that is archive state |
| `mcp_server`, `mcp_decision`, `mcp_state` | | Drift 1.3's MCP servers and approvals; read once by the importer, never written (servers now live in the engine's `mcp_config`, see [mcp.md](mcp.md)) |

Workspace icons are images only, downscaled client-side to a 64px webp data URL
(a few KB) before storage, so no blob handling or asset protocol is needed.

The first-launch import adds opencode projects as workspaces only when they own at least one session. Drift
also removes untouched temporary rows created by the older project-only importer while
preserving workspaces explicitly added through Drift, including empty ones.

The import runs at every start, so `imported_folder` remembers each opencode folder that is or was
a workspace, and the import never offers one again. Without it, a workspace the user removed came
back on the next start after the seven-day purge deleted its removed row. The purge records the
folder too, as it forgets the row, so a workspace removed before its folder was ever imported stays
removed as well.

## Archive lifecycle

Archiving never deletes: `archived_at` is set and the thread disappears from the
sidebar. On startup (once the engine is online) Drift purges archive rows older than
7 days and deletes those sessions from the engine. One archive drawer in the workspace
header lists archived threads and soft-removed workspaces together; restoring a thread
also restores its workspace when necessary.

## Workspace removal lifecycle

Removing a workspace is a soft delete (`removed_at`); its sessions are left completely
untouched (not archived, not deleted). Re-adding the same folder within 7 days restores
the workspace row, custom name and icon included, and all its threads are simply there
again. The startup purge hard-deletes workspace rows removed more than 7 days ago and
deletes all engine sessions in those directories.

## Access

- Frontend talks to typed commands only (`store_*` in `src-tauri/src/main.rs`); no SQL
  outside `src-tauri/src/store.rs`. Folder picking is a native dialog via `pick_folder`.
- `src/state/store.ts` is the single frontend facade. Workspace/archive methods use
  localStorage in browser dev; MCP methods fail explicitly because browser storage cannot
  provide the native execution boundary.
- `cargo test` covers the store roundtrip (upsert, rename, archive, purge, delete).
