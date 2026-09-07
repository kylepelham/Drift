# MCP approval backend

Drift-owned MCP definitions and execution decisions are global. They live in Drift's
native SQLite store and are not attached to a workspace or session. Workspace directory
is used only to locate the effective OpenCode report produced for that directory.

## Trust boundary

The Tauri shell materializes a generated OpenCode config under Drift's application data
directory. It references the read-only bundled `mcp-approval` plugin, a generation-matched
policy, and Drift-owned definitions. The vendored OpenCode tree remains unchanged; the
small `mcp-approval-guard.patch` overlay makes the plugin's final-config seal mandatory
before instance initialization when `DRIFT_MCP_APPROVAL_REQUIRED=1`.

The plugin receives OpenCode's final merged and substituted config. It immediately
replaces every MCP entry with the untyped `{ "enabled": false }`, computes canonical
SHA-256 fingerprints for valid local and remote definitions, writes a secret-free report,
and restores only exact approved fingerprints. The fingerprint covers the server name and
every effective definition field except top-level `enabled`, including command arguments,
URL, headers, environment, OAuth, timeout, cwd, and unknown effective fields. It does not
include directory, so an identical definition is approved everywhere.

Missing, malformed, stale, or unwritable policy/report state leaves all definitions
disabled. Schema-valid typed definitions that fail Drift's stricter transport policy are
reported as invalid and remain disabled without hiding valid definitions from the same
merged config. The overlay also rejects a missing, mutable, or replaced gate, any MCP mutation
after the approval hook, and all runtime `MCP.add()` calls in required mode. The sidecar is
authenticated with a random password and binds explicitly to `127.0.0.1`. Server auth
captures that password at listener-layer startup; bootstrap then removes it before config
substitution, and the plugin removes it again before stdio MCP children can inherit it.

OpenCode project and user plugins execute arbitrary code in the engine process. They are
outside this trust boundary: the approval gate prevents accidental or API-level MCP
bypass, not hostile code that can spawn processes or modify policy files directly.

## Decisions

SQLite stores decisions by immutable fingerprint. Editing or removing a server does not
delete old decisions: returning to an old exact definition restores its approval or
rejection. Rejection keeps execution disabled and lets clients suppress repeated prompts.
Revocation deletes only the exact fingerprint decision.

Approve, reject, and revoke require the current generation and an exact name/fingerprint
match. Drift uses the report for the requested directory when present; a stale, malformed,
or unreadable report is an error, not a reason to fall back. Only a missing report uses
configured definitions from Drift's registry and candidate config files across tracked
workspaces and global roots. This fallback keeps the first definition per name, with
registry entries first, and looks up decisions by fingerprint. It does not reproduce
OpenCode's config merging or variable substitution.

Approve/reject require `decision: "pending"`; revoke requires an approved or rejected entry.
There is no name-only approval. The plugin still validates and fingerprints the effective
definition, so a fallback approval cannot authorize a different merged or substituted definition.

## Reload protocol

Registry mutations, approval decisions, and detected external config changes share one
serialized protocol:

1. Atomically publish an empty next-generation policy and clear reports.
2. Call `POST /global/mcp/reload` to invalidate global and per-instance config caches, then
   close cached MCP clients and reap their stdio children without disposing engine instances.
3. Commit the SQLite mutation or generation advance.
4. Atomically replace generated config, then publish matching durable decisions.
5. Mark the generation materialized and remove any fail-closed sentinel last. Subsequent
   config/MCP access rebuilds the invalidated state and reruns the approval hook to write a fresh report.

The reload invalidates state rather than eagerly reconnecting every server. Closing MCP
connections can interrupt in-flight MCP calls, but does not dispose the sessions themselves.
Skill changes still call `POST /global/dispose`; neither path restarts the sidecar process.
External-editor saves/removals rewrite the matching config files first; the watcher then
performs the generation advance and reload above.

Generated files use same-directory temporary files and atomic replacement. Windows uses
`MoveFileExW` with replace-existing and write-through flags; the destination is never
deleted first. When the database has not changed, file recovery rematerializes its current
state. A failed post-commit materialization restores the previous registry data under a
fresh generation and rematerializes it. If recovery itself fails, Drift attempts every
fail-closed step: write the independently checked `mcp-fail-closed.json` sentinel, invalidate
the policy, clear reports, and invoke the same MCP-reload callback to stop active clients.
This is not a separate instance-disposal path. Incomplete shutdown is reported as an error;
successful materialization removes the sentinel last.

The live watcher hashes a bounded, deterministic set of config files, files below relevant
`.opencode/plugin` and `.opencode/plugins` directories, and `{file:...}` references found
in config text even when those references point outside watched roots. Parseable config files
are compared by canonical `mcp` and `plugin` content, so whitespace, comments, and unrelated
settings alone do not trigger MCP reloads. Other watched files and configs that cannot be
parsed within the size bound retain file signatures. A changed signature triggers the
serialized protocol above.

## Native API

`mcp_snapshot(directory)` returns `{ generation, directory, servers, observed }`.
`servers` are Drift-owned global definitions; `observed` contains only
`name`, `type`, `fingerprint`, and `decision` from the effective-config report or the configured
fallback described above.

All writes use the snapshot generation:

- `mcp_save(name, config, generation, previousName?)`
- `mcp_remove(name, generation)`
- `mcp_approve(directory, name, fingerprint, generation)`
- `mcp_reject(directory, name, fingerprint, generation)`
- `mcp_revoke(directory, name, fingerprint, generation)`

After a successful registry/decision write or the `mcp-config-changed` event, refresh runtime
metadata/status and fetch a new snapshot; do not dispose the instance to refresh MCPs.
External-editor writes rely on the watcher to complete the reload. Browser-only development
deliberately throws for all MCP registry operations because it cannot enforce the native policy boundary.

## Runtime recovery

An approved, enabled MCP connection that emits `client.onclose` is re-established with
exponential backoff from 500 ms to a 30 second cap. Retries continue until the transport
recovers, authentication becomes necessary, the user disconnects it, or the engine instance
is disposed. Explicit disconnect increments the connection generation before closing the
client, so that close event cannot revive a deliberately disabled server.

Ordinary MCP tool, prompt, and resource errors do not enter this recovery path. They remain
request failures while the client stays connected. OpenCode's existing one-shot expired HTTP
session recovery also remains active beneath this transport-close recovery.

While MCP management is visible, Drift refreshes runtime statuses every two seconds without
invalidating the exact-definition snapshot. The standalone `/mcp` dialog initially focuses
the Servers tab. Up/Down and Home/End move through servers, Left disconnects, Right connects
or authenticates, and Enter runs the selected server's primary runtime action.

## Validation

Drift preserves unknown fields while validating the complete current local and remote
schema when saving. OpenCode validates external config before plugin hooks; files that fail
that base schema surface an engine configuration error. After OpenCode merges valid external
config, Drift applies stricter checks to local
command/cwd/environment, remote URL/headers/OAuth, and shared enabled/timeout fields.
Remote URLs may use HTTP or HTTPS. Other URL schemes remain invalid. Invalid effective
transports remain disabled and produce a secret-free invalid observation alongside valid servers.
Configuration objects are size-bounded and reject unsafe object-property names.
