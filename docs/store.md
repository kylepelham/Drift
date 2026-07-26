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
| `workspace` | id, path (unique), name, icon, last_used, removed_at, purge_staged_at | workspaces = directories with display identity; purge staging keeps a removed path reserved until its exact session set is settled |
| `session_meta` | session_id, workspace_id, archived_at | Drift metadata about engine sessions; today that is archive state |
| `pending_session_delete` | session_id, directory, workspace_id, queued_at, claim, claimed_at | durable exact-ID engine deletion tombstones with opaque claims retained across startup, reconnect, and partial failure |
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
7 days by first recording exact-ID deletion tombstones. A worker must revalidate the
opaque claim, rotate it into an exclusive in-flight claim, delete the exact ID, and observe
that exact session as `404` before it can conditionally confirm the same claim. Failed attempts
release with a new claim; abandoned claims reset on process startup. Restore and claim are
serialized transactions, so restore either cancels pending work or is rejected while an in-flight
deletion owns the session instead of racing it. A stale snapshot can neither delete restored data
nor confirm a replacement
claim. One archive drawer in the workspace
header lists archived threads and
soft-removed workspaces together; restoring a thread also restores its workspace when
necessary.

## Workspace removal lifecycle

Removing a workspace is a soft delete (`removed_at`); its sessions are left completely
untouched (not archived, not deleted). Re-adding the same folder within 7 days restores
the workspace row, custom name and icon included, cancels its outstanding claims
transactionally, and all its threads are simply there
again. For workspaces removed more than 7 days ago, Drift snapshots the directory's exact
session IDs and reserves the removed row while deleting them. New sessions created after
path reuse are never selected by a path-wide delete. Session discovery paginates the
global endpoint to exhaustion before staging, and restoring the reserved path
cancels its workspace tombstones. The workspace row is hard-deleted only after every
staged ID is confirmed absent.

## Access

- Frontend talks to typed commands only (`store_*` in `src-tauri/src/main.rs`); no SQL
  outside `src-tauri/src/store.rs`. Folder picking is a native dialog via `pick_folder`.
- `src/state/store.ts` is the single frontend facade. Workspace/archive methods use
  localStorage in browser dev; MCP methods fail explicitly because browser storage cannot
  provide the native execution boundary.
- `cargo test` covers the store roundtrip (upsert, rename, archive, purge, delete).
