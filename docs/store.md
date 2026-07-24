# Drift store

Drift-side persistence lives in SQLite owned by the Tauri shell:
`%APPDATA%/dev.drift.app/drift.db`. The engine keeps its own storage; this database only
holds what Drift adds on top.

## Performance posture

- WAL journal, `synchronous=NORMAL`, 128MB mmap.
- One connection behind a mutex: single writer, no contention games.
- All statements go through `prepare_cached`.
- STRICT tables, indexes only where reads demand them (`session_meta.workspace_id`).

## Schema

| Table | Columns | Purpose |
| --- | --- | --- |
| `workspace` | id, path (unique), name, icon, last_used, removed_at | workspaces = directories with display identity; `icon` is empty (initials rendered from name) or a small data-URL image thumbnail |
| `session_meta` | session_id, workspace_id, archived_at | Drift metadata about engine sessions; today that is archive state |
| `mcp_server` | name, config_json, updated_at | global Drift-owned definitions; full JSON preserves unknown fields |
| `mcp_decision` | fingerprint, name, decision, decided_at | immutable global approval/rejection history by exact fingerprint |
| `mcp_state` | id, generation, materialized_generation | CAS and generated-policy publication state |

Workspace icons are images only, downscaled client-side to a 64px webp data URL
(a few KB) before storage, so no blob handling or asset protocol is needed.

Native startup imports OpenCode projects only when they own at least one session. Drift
also removes untouched temporary rows created by the older project-only importer while
preserving workspaces explicitly added through Drift, including empty ones.

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
