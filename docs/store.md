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
| `workspace` | id, path (unique), name, icon, last_used | workspaces = directories with display identity; `icon` is empty (initials rendered from name) or a small data-URL image thumbnail |
| `session_meta` | session_id, workspace_id, archived_at | Drift metadata about engine sessions; today that is archive state |

Workspace icons are images only, downscaled client-side to a 64px webp data URL
(a few KB) before storage, so no blob handling or asset protocol is needed.

## Archive lifecycle

Archiving never deletes: `archived_at` is set and the thread disappears from the
sidebar. On startup (once the engine is online) Drift purges archive rows older than
7 days and deletes those sessions from the engine.

## Access

- Frontend talks to typed commands only (`store_*` in `src-tauri/src/main.rs`); no SQL
  outside `src-tauri/src/store.rs`. Folder picking is a native dialog via `pick_folder`.
- `src/state/store.ts` is the single frontend facade. In browser dev (no shell) the
  same interface is backed by localStorage; dev-only, no performance claims.
- `cargo test` covers the store roundtrip (upsert, rename, archive, purge, delete).
