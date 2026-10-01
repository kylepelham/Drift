# UI completion audit at bc2a8cd

Source audit on 2026-10-01, rechecked at `bc2a8cdd1`. The user requested an
audit of features whose backend implementation was treated as end-to-end
completion. Browser verification was stopped at the user's request. No runtime
source changes or fresh post-fix UI smoke results are claimed here.

## Reasoning picker: original missing wiring is fixed

`eb1a408ea` supplies catalog variants through `native/adapt.ts`; the composer
can now enumerate them. `4f5d1f3ad` sends named variants, carries them on the
session and passes them through retry model switches. The old missing-variants
and dropped retry-variant findings must not be handed back as still open.
UI-to-wire acceptance coverage is still the appropriate completion gate.

## Source-confirmed gaps

| Feature | Broken boundary | Consequence |
| --- | --- | --- |
| Composer agent selection | Composer passes `selectedPrefs.agent`, but `actions.send` drops it and `newSession` creates without an agent. The session API defaults to `build`. | Plan/custom-agent selection does not select the executing agent. Plan's backend dispatch restrictions do not protect a session still running Build. |
| Agent identity in the UI | `adaptMessage` hardcodes the user agent and assistant mode to `build`; `adaptSession` omits the native agent. | History does not preserve actual agent identity. The orchestrator driver checks the adapted goal agent, so it cannot reliably recognise native orchestrator turns. |
| MCP management | Manager mutations go through `driftStore` and Tauri `McpRuntime`, writing `mcp_server`/`mcp_decision` and legacy `opencode.json`. Runtime status/connect go to the native API, which uses `mcp_config`. | Add/edit/approve/remove and runtime control refer to different configuration authorities. The native save/approve client methods exist but are not called by the manager. |
| Agent behavior editor | Settings accepts raw behavior JSON; shell validation accepts steps, variant, temperature, tools and permissions. `AgentOverride` reads and applies only model and prompt. | Unsupported fields can be saved and displayed without affecting execution. Native agent projection also omits fields such as steps and replaces tools/permissions with placeholders. |
| Family system-prompt editor | `family:*` overrides are saved and materialized for legacy plugins. Native override forwarding filters for `agent:*`, and native prompt assembly never reads family overrides. | Saving a family prompt does not change the native model's system prompt. |
| Archive/restore | Sidebar, `/archive` and restore call shell `session_meta` operations. They do not call the native session archive endpoint. | Archiving hides a thread but does not mark its native session archived or invoke the native archive Stop path. |
| Removed-workspace expiry | `removeAllSessions` is a stub returning false; the purge coordinator relies on it. | Seven-day removed-workspace cleanup never completes. |
| Transcript search and storage inspection | Shell search/storage use `engine_db::database_path`, targeting OpenCode's database and schema. | New native transcripts are outside those retained UI features. This needs an explicit cutover/retained-feature gate. |
| Bounded fork | Native client supports `atMessage`, but the action and visible slash/sidebar flows never pass one. | Bounded fork is an API feature, not a reachable UI operation. |
| UI submission retry | Each `send` generates a new submission id; the submission guard retains no retry identity after failure. | If the engine accepts but its receipt is lost, resending the retained draft uses a different id and defeats backend idempotence. |

The archive, search, storage and cleanup paths are retained-feature/cutover
gaps. They should not be represented as proof that every native UI feature is
complete, nor confused with the correctly implemented backend operations.

## User-requested Settings removal

Remove JEV/tool routing from Settings. `settings.tsx` still mounts
`ToolRoutingSetting`, whose state calls legacy shell routing APIs and polls
status. The native engine has no corresponding routing implementation.
Remove the obsolete UI binding and polling rather than leaving a toggle that
appears to configure native execution.

## Acceptance requirement

For each exposed control, verify selection/save, the action payload, durable
engine state, effective execution and reload behavior. Backend tests alone
remain useful but do not establish those UI contracts. Narrow unsupported
controls or mark them explicitly pending instead of reporting saved/applied.
