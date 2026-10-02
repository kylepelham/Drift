# Engine rewrite: reviewed proposal

Separate proposed revision of `docs/engine-rewrite.md`, reviewed 2026-09-29 on
`next/1.4.0-engine` at `24ca7164ee5519fbb98138eda16452213b3c597c`. The original plan
and `CHECKLIST.md` remain the plan and milestone status of record. Updated against
the subsequent uncommitted implementation on that same date: HEAD alone does not
identify the audited code. This copy records verified progress and remaining work;
it does not mark entire milestones complete or change the implementation.

**Latest M1 verification at `a0fce77d`:** the agreed M1 correction list and the
recorded Anthropic conformance suite pass code/automated review. See
[the sign-off follow-up](research/m1-signoff-review.md#follow-up-at-a0fce77d).
The desktop smoke test remains explicitly pending; later-phase architecture work
is not being held up by the older correction lists. The
[896deaf checkpoint](research/checkpoint-896deaf.md),
[prior fix review](research/fix-review-b99a6ec.md) and
[initial progress review](research/progress-review-772fe5c.md) preserve earlier evidence.
The M0 snapshot below is historical, not an instruction to rebuild current work.

**Review scope clarification:** the user wants the first phase signed off so later
phases can proceed. Use [the M1-scoped review](research/m1-signoff-review.md) as the
immediate completion list. The broader recommendations here are later design
inputs, not a demand to finish profiles, MCP, subagents or migration before M1.

The chosen architecture stays: a clean-room Rust engine library linked into Tauri,
one `drift.db` writer, HTTP plus one WebSocket, generated frontend types, native
provider transports, and no JavaScript engine-plugin host. The amendments below
come from the [research dossier](research/README.md), including the installed
Claude Code binary, current Drift/OpenCode code and official provider contracts.

## Earlier M0 implementation snapshot

At this earlier inspection the rewrite was partway through M0, not waiting for its
first code. Keep the working foundation and finish its contracts. This historical
status is from source inspection
and the checks at the end of this document, not an installed-app UI smoke test.

| Area | Observed implementation | Status and remaining work |
| --- | --- | --- |
| Workspace and linked library | Root `Cargo.toml` includes `drift-engine`, `drift-engined` and `src-tauri`; Tauri calls `native::start` during setup. | Present and workspace tests pass. Retain this topology. |
| Headless launcher | `crates/drift-engined/src/main.rs` launches the same library and can export its OpenAPI. | Present; use it for conformance and generation rather than creating another runner. It is not a production sidecar. |
| Native API | `crates/drift-engine/src/api/mod.rs` registers authenticated health, OpenAPI, workspace list/create and WS routes. | Functional foundation with API tests. Native sessions, providers and turn routes are not implemented. |
| Workspace persistence | `store/mod.rs` uses WAL, prepared statements, path lookup and restore/touch behavior. `store/migrations.rs` has one numbered workspace migration. | Implemented narrowly. It is not yet a populated-shell migration suite or a session/replay schema. |
| Writer ownership | `src-tauri/src/main.rs` opens the native engine and then opens the shell store. Each owns its own `Mutex<Connection>` against the same `drift.db`. | Shared file, not shared writer. Consolidate before adding authoritative session writes. |
| Event hub | `event.rs` has a 4,096-event ring; attach subscribes and selects replay under one lock. `api/events.rs` sends `hello`, replay, `resync`, and handles broadcast lag. | Useful same-process replay already exists. Restart identity, future cursors, durable outbox, revisioned snapshots and replies remain open. |
| OpenAPI generation | `scripts/gen-engine-client.ts` generates `src/engine/native/types.ts`; `tests/engine-client.test.ts` checks it against fresh OpenAPI. | Generated schemas exist. `client.ts` is a handwritten typed request wrapper; generation is a manual `gen:engine` command, not part of build/dev scripts. |
| Native frontend connection | `src/engine/native/target.ts`, `client.ts` and `events.ts` resolve the target, fetch health and reconnect WS. | Transport foundation exists. HTTP cancellation, target/token replacement and asynchronous snapshot handoff still need contracts/tests. |
| Actual UI integration | `src/engine/index.tsx` records native version/online state, with empty native hydration and event handlers. Settings displays diagnostics. | Diagnostic integration only. Workspace state and chat still use shell/OpenCode paths; do not label this a native chat cutover. |
| Startup and exit | Tauri starts native and legacy engines; `scripts/dev.ts` also starts both, with a separate temporary native dev database. Native stop aborts the server task. | Both start by default. Add coordinated bind/stop and socket shutdown tests; there is no persisted native-enable toggle. |
| Authentication and remote | Native HTTP uses bearer auth, browser WS uses a token query. The existing remote gateway still proxies OpenCode. | Local token protection exists. Native origin policy, credential replacement and remote HTTP/WS mounting remain future work. |
| Native agent functionality | The engine library currently exports API, events and workspace store modules. | No native provider adapter/auth flow, session runner, tool execution, MCP, compaction, permission/question service or migrator was found in the audited tree. These remain new work. |
| Validation | Rust API/hub/store and native frontend tests exist and pass in this follow-up. | The previous reqwest `No provider set` failure is resolved in the current test harness, not a current blocker. |

The original six M0 checklist entries therefore break down as follows: workspace
and linked API are implemented; WS replay, storage and frontend client integration
are partial; the requested comparative performance baseline is not yet recorded
in the audited implementation. `CHECKLIST.md` remains unchecked and is not used
as evidence that no code exists.

### Next changes to the existing foundation

1. **Unify the writer first.** Reuse the new store work, but make shell and engine
   share its connection owner and migration ledger. Move shell operations without
   losing archive, removed-workspace, MCP, prompt or remote-device state. Test an
   existing shell database, not only a fresh in-memory workspace table. The current
   `CREATE TABLE IF NOT EXISTS` does not convert an existing non-STRICT table to
   STRICT; choose an explicit compatibility or rebuild migration.
2. **Finish the event contract in place.** The hub starts again at sequence 1 on
   `Engine::open`. `Ring::since` currently treats `cursor >= head` as caught up,
   including a cursor from a prior process. Separate equality from a future cursor
   and resync the latter; update the existing `attach_at_head_replays_nothing` test,
   which currently asserts that a future cursor is accepted. Define persistent
   sequence allocation and instance/credential replacement before sessions depend
   on replay. Keep the already-correct atomic subscribe/replay attachment.
3. **Make hydration real and ordered.** `hydrate(seq): void` currently advances the
   client cursor before a synchronous callback, and app callbacks do nothing. Add
   an awaited revisioned snapshot handoff, buffering or reconciling concurrent
   events, then advance the applied cursor. Test slow/failed hydration, reconnect
   during hydration, duplicates, a restarted engine and a changed target/token.
   A fake callback that stores a number is not a state-recovery test.
4. **Complete the generated API boundary.** Keep the current OpenAPI and schema
   drift test. Complete the plan's generated request client or explicitly approve
   a thin maintained transport wrapper over generated operations; do not claim a
   handwritten wrapper is generated. Integrate generation/checking into the chosen
   build/CI path and add `AbortSignal` support before long-running operations.
5. **Close lifecycle and local/remote gaps.** Preserve bearer auth. Restrict query
   credentials to the WS/bootstrap contract, define allowed origins, and ensure
   tokens are not logged. Coordinate the asynchronous bind task with shutdown so
   a late bind cannot outlive Stop. Test active-socket termination and target refresh.
   Native remote mounting remains separate from the legacy proxy.
6. **Start M1 on durable input and replay contracts.** Add admission, attempts,
   tool outcomes and native provider blocks through the shared writer, with a fake
   provider first. Do not extend workspace CRUD into a chat-part-only model and
   retrofit these boundaries afterward. Introduce hook/config interfaces as their
   first M1 consumers arrive, without building every later service just to finish M0.

Workspace creation currently commits through the store and separately calls
`hub.publish` in `api/workspaces.rs`; that is not a transactional outbox. Preserve
the working route, but do not reuse that two-step pattern for admitted turns,
accepted answers or tool outcomes that require recoverable event publication.

## Changes from the original

Original line references refer to the 189-line plan at the commit above.

| Original section | Proposed amendment | Reason |
| --- | --- | --- |
| Providers, lines 26, 134-147 | Native xAI Responses adapter and its own access profiles in M2, alongside OpenAI; retain Gemini and later cloud routes explicitly. | xAI encrypted reasoning, continuation, hosted tools and supported fields differ from OpenAI. |
| Auth, line 28; risk, lines 182-183 | Treat plugin and first-party source as evidence, not the OAuth specification. Own account, auth, refresh, wire profile, catalog and quota behavior. | The Anthropic plugin changes request identity, tools and headers; replacing it is larger than PKCE or one file. |
| Catalog, line 27 | Keep models.dev for discovery, add reviewed route/model capability profiles and provenance. | A catalog does not establish subscription entitlement or wire compatibility. |
| Dropped, line 31 | Distinguish a named vendor gateway integration from generic gateway access. Preserve user-configured gateways, OpenRouter and local endpoints. | The user requires every access mode natively. |
| Data model, lines 100-106 | Reuse actual existing shell tables, design a new blob store, add durable input/attempt/tool/ask/context records. | The shell has no `blob` table; `workspace` and `mcp_server` already exist. Chat parts alone lose execution and provider replay state. |
| M0 storage | One shared writer and migration owner before opening engine tables. | Separate mutexes around separate SQLite connections are still multiple writers. |
| API, lines 81, 94-98 | Submission idempotency and committed admission receipt; explicit provisional events, reply receipts and revisioned resync. | HTTP acceptance and a WS ring do not by themselves survive crashes or lost acknowledgements. |
| Tools, lines 30, 32-34 | Resource-aware scheduling, content-version checks, truthful formatter outcomes and scoped snapshots. | Concurrent stateful tools and external file edits can invalidate apparently correct model actions. |
| M3/M5, lines 146, 157-160 | Config generations and minimal typed Hook contract move into foundations/M1 before MCP and reload. | Approval and resource lifetimes must not depend on a seam introduced after cutover. |
| Layout, lines 63-65 | Keep the old engine through M4, not only M1. | M1 is one vertical slice, not provider, migration and workflow parity. |
| Cutover, lines 149-155 | Phased, restart-safe migration with actual source-path inventory, archive semantics, credential handover and rollback. | Legacy databases, vault secrets and files cannot move in one SQLite transaction. |
| Testing, lines 120-121, 169 | Deterministic CI gates plus matched live task-quality and latency evaluation. | First event is not visible text; a faster incorrect task is a regression. |

## Why

- The 28 build overlays encode product behavior and accumulated race fixes. Replace
  their invariants with owned services and regression tests, then remove the patches.
- Deep support for a small set of model families should preserve their native
  reasoning, tool, caching and continuation semantics rather than flattening them
  into a common chat SDK.
- A linked engine and single writer simplify ownership, deployment and migration.
  Reduced startup or turn latency remains a measured goal, not an architectural
  guarantee.
- Drift should own config names, APIs, data lifecycle, prompts and authentication
  without silently changing the user's billing account or dropping subscription access.

## Decisions

| Area | Reviewed decision |
| --- | --- |
| Approach | Independently authored Rust implementation. Existing engines and public protocols inform requirements and tests; do not copy their implementation into the new core. Raw proprietary binary/source remains outside git. |
| Topology | `drift-engine` library linked into Tauri. `drift-engined` launches the same library for headless use and conformance. No production engine sidecar. Tool and MCP subprocesses remain supported; they are not engine sidecars. |
| API | Owned axum HTTP API and one WS. Generate OpenAPI and frontend client/types. Never hand-maintain a second DTO contract. |
| Providers | First-class native Anthropic Messages, OpenAI Responses, and xAI Responses. Explicit Chat Completions compatibility where required. Retain Gemini; Bedrock and Vertex are route/credential integrations, not anonymous equivalents of the direct endpoints. |
| Access modes | Native direct keys, Claude subscriptions, ChatGPT/Codex subscriptions, existing SuperGrok path, named gateway profiles and local endpoints. No OpenCode/npm authentication plugin at runtime. Each route has independent auth and conformance gates. |
| Gateways/local | Preserve OpenRouter and configured passthrough/compatible gateways; retain Zen when selected independently of OpenCode core. LM Studio and Ollama have runtime/model discovery and tested endpoint profiles. Z.ai remains an explicit compatibility profile. |
| Catalog | models.dev cache and bundled fallback seed discovery. Reviewed capability records add protocol, route, auth mode, model/instance, tool profile, reasoning/replay, schema, cache and context limits with source and revision. Unknown capability is not assumed supported. |
| Auth | Rust auth service shared by linked/headless entrypoints. Platform vault backend, one metadata writer, account generations, coordinated refresh and truthful quota state. Define headless fallback key protection before promising encrypted-file support. |
| Hooks | Minimal internal Rust `Hook` trait and serde contracts before features consume it. No JS host. MCP approval remains a mandatory core gate, not a plugin that can be removed. Public compiled-plugin ABI remains deferred. |
| Tools | Keep the original `read`, `edit`, `write`, `apply_patch`, `bash`, `glob`, `grep`, `webfetch`, `todowrite`, `skill`, `question`, `task`, `spawn_thread`, `read_thread` list. The shell name is a compatibility label; its contract must describe the actual platform shell. |
| Dropped | Keep `websearch`, `lsp`, `execute`, `plan` tool, public share hosting, ACP, TUI and interactive CLI dropped unless separately reconsidered. Keep the original nonpriority vendor integrations dropped. This does not drop the headless runner, plan agent, provider-hosted search, or generic gateway access. |
| Edit | Exact match with line-ending normalization. Closest-region output is diagnostic only, never a fuzzy replacement. Content-version and resolved-path checks precede mutation; `apply_patch` profile is catalog-selected. |
| Post-edit | Formatter hooks without LSP. Capture final bytes/version and formatter outcome. Failure is nonfatal when appropriate but cannot be hidden from tool result/audit if it changes correctness or final file state. |
| Snapshots/revert | Keep shadow-Git snapshots, scoped by worktree and operation, with dirty-user-state preservation and declared exclusions. Measure cost and batch related writes where safe. Snapshot support is not rollback for remote MCP effects. |
| MCP | Native `rmcp` after transport/OAuth capability verification. Approval on final effective config before connection, bounded reconnect/catalog readiness, resource-aware scheduling and revision-bound clients. |
| Storage | One existing-path `drift.db`, one shared writer and migration owner, WAL and explicit durability policy. Engine tables live beside extended shell tables. |
| Config | Keep `drift.json`, `.drift` directories, conventional instructions/skills and no runtime `opencode.json` fallback. One-time importer reports unmapped keys and unsupported plugins. Running jobs lease immutable effective generations. |
| Identity/paths | `DRIFT_*` names. Preserve the current Tauri-resolved database path for upgrades. Any move to another platform data directory is an explicit migration, not a blind rename. |
| Permissions | One allow/deny/ask protocol; schema-validate hook edits before the final permission decision. Session always rules and agent overrides stay explicit. Live revocation can stop dispatch without changing a run's pinned definitions. |
| Session tree | `parent_id` and hidden/sibling visibility. Persistent child jobs and receipts; explicit model/account/context/cancel inheritance. Tree operations share admission coordination across all clients. |
| Platforms | Windows first, with macOS/Linux CI. OS path identity, shell/process-tree control, vault access and file replacement live in `platform`. |

The provider matrix in [provider-contracts.md](research/provider-contracts.md)
separates required product support from feature availability on a particular
model, account or route. A failed subscription route never falls through to a
billable API-key route without explicit account selection.

## Layout

```text
Cargo.toml
crates/drift-engine/
  src/
    api/                   axum HTTP, WS sequencer/ring, OpenAPI
    session/               admission, runner, jobs, context, compaction, snapshots
    llm/                   Provider, native adapters, catalog, auth, prompt recipes
    tool/                  Tool, registry, profiles, scheduler, implementations
    edit/                  exact matcher, apply_patch, versions, formatters
    mcp/                   transports, approval, reconnect, OAuth resource client
    config/                generations, agents, commands, skills, instructions
    permission/ question/ hook/ store/ platform/
crates/drift-engined/       launcher for the same library
crates/drift-migrate/       one-time legacy import and report
src-tauri/                 links engine; host platform services, TLS/device auth
src/engine/                generated client, WS pump, projections and thin actions
tests/conformance/         Bun black-box suites with deterministic providers/tools
```

One engine library until build measurements justify a split. The library and
headless launcher already exist. Keep the vendored engine frozen until M4.
Current desktop and browser development start both engines by default; native
only reports diagnostics in the UI. Comments saying this ends at M1 do not replace
the provider/migration cutover gates. A temporary adapter for currently consumed legacy
routes/events may live at the new API boundary during transition; it cannot own
auth, tools, policy or a second runner. Remove it once the UI uses generated
Drift types. Do not ship a permanent OpenCode compatibility engine.

## API and events

Retain the implemented health, workspace and event foundation. Catalog, session,
MCP and search routes from the original plan are still proposed, not current API
coverage. Extend the existing OpenAPI with these contracts as they are implemented:

```text
POST /sessions/{id}/turns
  {submission_id, parts, profile_id, model, agent, variant, delivery: steer|queue}
  -> 202 {input_id, turn_id?, admitted_seq, disposition}

POST /sessions/{id}/abort
  {operation_id, expected_run?}
  -> {cancel_generation, state: stop_requested|quiescent|outcome_unknown}

GET /sessions/{id}/snapshot
  -> {revision, cursor, messages, jobs, asks, status, context_revision}

WS /events?cursor=
  -> {seq, type, session_id?, revision?, attempt_id?, durability, payload}
  <- {request_id, type: permission.reply|question.reply, ask_id, payload}
  -> reply acknowledgement with request_id, durable resolution or typed conflict
```

Admission commits before its 202 response. Exact retries with the same submission
ID and payload return the original receipt; conflicting reuse fails. Execution
starts separately. A WS timeout does not imply an answer failed to save. Accepted
clarification replies preserve owner/model/context and are queued without
interrupting unrelated active work. Permission and plan approval remain blocking.

All events have monotonic `seq`. Do not reuse sequences after process restart.
Durable control events publish after their state transaction commits. Provisional
text identifies its attempt and may be checkpointed in batches; it is not a
committed final response. Commit or invalidation resolves that projection.

The ring is a bounded delivery cache. Expired cursor, missing retained events or
restart returns `resync`; the client replaces relevant state from a revisioned
snapshot and resumes without a snapshot/live-event race. Slow subscribers
disconnect instead of growing unbounded queues. Test invalidation lost during
disconnect, heartbeat-only stalls and cross-workspace ownership.

Local loopback still requires per-run authorization and allowed-origin checks.
For remote access, mount the same router and WS upgrade behind Tauri's existing
TLS, device-token, host/origin and management-route policies. Bind sockets to the
device auth revision and close them on revoke/disable. Removing the old HTTP
proxy must not remove these protections or assume it already implements WS.

## Storage and recovery

### Existing tables are migration inputs, not empty names

The current shell already owns `workspace`, `session_meta`, `mcp_server`,
`mcp_decision`, `mcp_state`, `prompt_override`, `app_setting`, `remote_access` and
`remote_device` in `src-tauri/src/store.rs:84-149`. There is no shell `blob` table.
Current media can live as data URLs in file parts. Plan a new content-addressed
blob store with references, migration and garbage collection.

Use one writer service/connection owner for shell and engine operations and one
numbered migration ledger. Do not open another `Store(Mutex<Connection>)` against
the same file and call that single-writer. This is a concrete next change: native
startup currently does exactly that before the shell opens its own connection.
The existing engine migration handles workspace only. Upgrade fixtures must include removed
workspaces, archived sessions, MCP approval generations, prompt overrides and
remote-device records. Preserve existing IDs and optional fields.

### Add execution records, not just display parts

- Durable inputs with submission/payload identity and steer/queue state.
- Turns with pinned account, model/capability/config/agent revision and cancel generation.
- Provider attempts with request fingerprint, partial/complete status and raw usage.
- Ordered native replay blocks/items preserving signatures, ciphertext, IDs,
  references and unknown replay-relevant fields independently of UI projections.
- Tool execution intents, effective arguments, permission receipts, scheduling
  claims, outcomes and result references.
- Durable permission/question requests and idempotent answer records.
- Context revisions with retained history, omission reasons, summary provenance,
  discovered tools and file evidence.
- Transactional control-event outbox, snapshots and import/cutover ledgers.

Tool mutations require a complete, validated provider response and permission
receipt. Optionally speculate only certified non-effectful reads after a complete
call block, with distinct provisional identity and no early permission prompt or
effectful hook. Discard them with a failed response. MCP read-only hints alone do
not certify speculation safety.

If a process dies after an external effect but before its result commits, record
`outcome_unknown`. Reconcile a remote receipt or supported idempotency key before
retrying. There is no universal exactly-once guarantee for arbitrary external
tools. Stop fences new dispatch, requests cancellation and reports acknowledged,
timed-out or unknown outcomes; it does not claim remote work was rolled back.

Pin config and capability definitions for the whole run. A deliberate retry-model
switch records a new compatible context lineage. Same-provider does not guarantee
cross-model replay compatibility. Never reconstruct signed or encrypted reasoning
from visible text or send another account's continuation handle to a new route.

Archive retention remains seven-day, two-phase purge. Preserve the distinction
between Drift's archive tombstones and imported OpenCode `time_archived`. Move
purge authority out of UI timers into the shared service. Delete confirmed session
data and dereference blobs/snapshots before clearing tombstones; restored
workspaces cannot be purged by a stale sweep.

## Tools, context and native login

The scheduler is bounded across sessions and uses resource claims for files,
browser sessions, desktop targets and other stateful integrations. Unknown/mutating
calls are conservative; unrelated safe reads can overlap. Acquire resource sets
atomically and define fairness and cancellation. One runner per session is not
sufficient to protect a shared MCP desktop target.

Edits validate content version and resolved path under an engine path lock,
preserve encoding/line endings and replace atomically where supported. Failed
replacement must not silently become a partial direct write. External writers can
still race a filesystem lacking conditional replacement; report that limitation.
Revert targets recorded tool changes and preserves unrelated dirty user files.
Snapshot exclusions for ignored/large/out-of-workspace files and non-Git workspaces
are explicit. Shell and remote MCP writes have different recovery contracts.

Add local per-tool discovery and persistent discovery state. Prefer native deferred
schemas when the route supports them; otherwise an explicit next-request tool-set
change is allowed and its cache cost is measured. Tool budgets, previews and
retrieval handles must declare truncation. The plan still drops Jev and `execute`;
validate local-model tool success against the old code-mode baseline before cutover.

Context accounting comes from the prepared request, with estimated categories
separate from actual usage. Keep original transcript evidence while compaction
publishes a new request-view revision. Preserve complete call/result pairs, active
tasks, discoveries and user constraints. Failed summary never replaces working
context. Optional background memory extraction requires quality/cost evidence.

Native authentication includes browser/device state, scopes, callback ownership,
account selection, token rotation, wire headers, model eligibility and usage.
The [auth report](research/native-auth.md) documents current implementation behavior
and remaining access-contract uncertainty. Plugin tarballs and compiled client IDs
do not establish a provider-supported third-party integration. Subscription access
is a required route with its own release gate, not a promise synthetic fixtures
can fulfill. Keep keys, subscriptions, gateway tokens and MCP OAuth separate.

Refresh has one local owner per account/generation and re-reads under that ownership.
Persist rotated credentials before making them available. CAS prevents stale local
writes; it cannot make issuer rotation and local vault/SQLite writes transactional.
Handle ambiguous refresh and logout races without silently corrupting the account.
Never expose credentials through generated DTOs, WS, model context or child env.

## Milestones

These are proposed refinements of the original milestone criteria, now accounting
for implementation already present. The existing `CHECKLIST.md` is unchanged.
The status above is evidence for individual pieces, not an entire milestone signoff.

### M0: complete the existing foundation

- Retain the implemented workspace, linked library, headless launcher, loopback
  API and basic auth. Their Rust tests now pass; do not recreate these pieces.
- Consolidate the currently separate stores into one writer/migration owner and
  prove populated-shell compatibility, including removed/archived/MCP/remote state.
- Extend the existing ring/client with restart-safe sequence identity, future-cursor
  resync, target replacement and real revisioned snapshot handoff. Add lifecycle
  shutdown and origin/credential-scope tests to the existing API suite.
- Complete the generated API boundary from the present schemas/typed wrapper,
  integrate generation/drift checks into build/CI, and apply native workspace
  events to real state without replacing chat before its M1 gates pass.
- Agree admission, attempt, tool-outcome and provider replay schema before M1
  storage is implemented. Agree hook/config ownership now; implement their first
  consumers with the vertical slice rather than requiring all M2 services in M0.
- Preserve the resolved test TLS initialization and check linked/headless lifecycle
  behavior separately. Passing API tests is not an installed-window smoke test.
- Record the still-missing baseline for startup, preparation, first event/text,
  tool waits, completion and schema/cache usage. Separate legacy-only cost from
  the current dual-engine development startup so the comparison is meaningful.

### M1: recoverable Anthropic vertical slice

- Begin from the existing API/store/hub, with fake-provider and fake-auth fixtures
  at the owned boundaries. Add provider/MCP/stateful-tool fixtures alongside their
  first consumers; no live credentials in ordinary tests.
- Native Messages text, thinking, tools, usage and fragmented streaming fixtures.
- API-key and owned Claude subscription auth implementation, with separately
  recorded live login/route conformance before real subscription cutover.
- Turn runner, exact-retry admission, permissions, post-hook input validation,
  cancellation and restart reconciliation.
- Minimal typed Hook contract and immutable run-config leases used by the runner,
  permissions and prompt preparation before later MCP/reload integrations.
- `read`, `edit`, `write`, shell, glob and grep with version evidence, scoped
  checkpoints, output retrieval and bounded resource scheduling.
- UI works through generated API/WS, including failed attempts, invalidated
  provisional output and pending permission recovery.
- Deterministic crash tests before/after admission and tool effect/result boundaries.
- Existing engine stays available as a development baseline; no global cutover.

### M2: priority providers, MCP and configuration

- Native OpenAI Responses, API key/Codex auth, `apply_patch` profile and lossless
  reasoning/item replay; Chat compatibility only through explicit endpoint profile.
- Native xAI Responses with its own decoder/capabilities/cache/replay and owned
  SuperGrok access path where verified. It is not an OpenAI preset.
- Named gateway and LM Studio/Ollama profiles with loaded-instance context and
  native/compatible protocol conformance. Preserve user-required gateway access.
- MCP stdio/HTTP/OAuth with effective-config approval before connection, revision
  leases, reconnect, bounded restoration and shared-resource scheduling.
- Config/agent/command/skill parsing and prompt overrides with immutable running
  generations. No live OpenCode config or JS plugin execution.
- Todo, skill, question, webfetch, durable async replies and formatter outcomes.
- Per-tool discovery and route-specific cache accounting.

### M3: lifecycle and remaining selected routes

- Hidden/sibling child jobs, `task`, `spawn_thread`, `read_thread`, explicit
  permissions/model/account/budget/cancellation inheritance and persistent receipts.
- Active/full bounded fork, move with atomic tree-admission gate, revert/diff
  preserving dirty user files, shell timeout and truthful process cleanup.
- Compaction/recovery, retained native context, retry-model lineage switching and
  engine-owned autonomous continuation that survives UI disconnect.
- Gemini, Bedrock, Vertex and Z.ai remain explicit planned integrations. Define
  supported cloud credential chains and avoid hand-written signing/auth behavior
  without official vectors and conformance. Their work does not delay validation
  of the three priority native families until M3.
- Every carried behavior in the [28-overlay ledger](research/opencode-exit-inventory.md#overlay-ledger-all-28-patches)
  has a new-core regression scenario or an explicit dropped-feature disposition.

### M4: migration and cutover

- `drift-migrate` inventories actual shared/channel OpenCode DBs, shell metadata,
  attachment locations, config and auth provenance before importing.
- Source snapshots are WAL-consistent. Imports preserve IDs, unknown historical
  parts and provenance, archive joins, prompts and approval state. Repeat import
  is idempotent; malformed historical parts cannot poison active context.
- Existing shell data stays in the Tauri-resolved `drift.db` path. New blob storage
  imports data URLs/artifacts with deduplication and reference-aware GC.
- A persisted cutover ledger covers ingress fencing, stopping old writers,
  source snapshot sealing, staged import, authority selection and verification.
  Exercise restart at every phase. Vault and file changes have recoverable receipts.
- Before new writes, rollback may discard staging. After new sessions or credential
  rotation, preserve/forward-export that state rather than reopening stale backups.
- Real conformance for every required subscription/API/gateway/local access profile
  and feature parity gates pass before retiring its current route.
- Remote HTTP and WS share the engine router behind existing TLS/device policy;
  revocation terminates active streams and management scope stays restricted.
- Remove OpenCode SDK, runtime, overlays, generated prompts, auth plugins and build
  scripts only now. No direct legacy database readers or plugin downloads remain.
- Keep required notices for retained licensed material; raw Claude source is not
  part of the distribution. Publish measured results and verification limits.

### M5: hook refinement, not first security integration

- Stabilize and document internal Hook contracts already used by M1/M2.
- Test hook deadlines, ordering, failure isolation and post-hook validation.
- Prompt customization can use hooks; mandatory MCP approval and permission
  enforcement remain non-bypassable core operations.
- No JS host or public compiled-plugin ABI is added implicitly.

## Testing and performance gates

Use Rust unit tests plus Bun HTTP/WS conformance against `drift-engined`. Mine
reference code for scenarios, not copied test implementations. Public/synthetic
wire fixtures must include partial UTF-8/JSON, signed/empty reasoning, parallel
calls, unknown events, cumulative usage, cancellation and interrupted streams.

Inject failures at admission commit, provider send, tool effect, result commit,
event publication, auth rotation and each migration phase. Require no automatic
retry of uncertain mutations, no credential/account crossover, correct resync,
and replay-critical fidelity. Remote idempotency applies only where the remote
operation supports it.

CI checks deterministic overhead, allocations/queue bounds and contract correctness
with pinned fixtures. Live provider comparisons are separate paired tasks with
model, effort, account route, tool catalog, budget and cache state recorded.
Measure time to a correct completed task as well as first visible text. Do not
fail ordinary CI because an external model was slow today, and do not claim Rust
or the absence of IPC guarantees a particular speedup.

Run `bun run typecheck`, engine unit/conformance tests and workspace clippy as
applicable. Research inspector scripts have their own strict TypeScript check
because the application tsconfig does not include `scripts/`.

## Effect on the app

- `src/engine/native` already contains schemas, typed HTTP transport, target lookup
  and WS reconnection. Extend those modules into stateful integration, then replace
  legacy SDK/SSE use. The current Settings indicator and empty native callbacks
  are not chat migration. UI state stays one-way; components gain no raw API calls.
- Shell and engine share the writer, preserving workspaces, archive retention,
  MCP decisions, prompt overrides, remote devices and settings.
- Old `engine.rs` process control and `engine_db.rs` runtime merge retire after
  migration. `usage_limits.rs` behavior moves behind owned auth/usage services;
  deleting its legacy credential reads does not delete the usage feature.
- Remote proxy code becomes authenticated router mounting and WS upgrade, retaining
  device revocation, host/origin checks and desktop-only management boundaries.
- Do not overwrite concurrent Rust scaffold work while adopting this proposal.

## Remaining decisions before implementation is considered complete

1. Provider-approved subscription access and live entitlement remain separate
   from recovered OAuth mechanics. Which compatible routes actually pass must be
   recorded per account/profile before cutover.
2. Define the OS-vault and headless fallback durability/key ownership contract.
   Secret vault changes and SQLite metadata need reconciliation after a crash.
3. Finalize snapshot coverage and cost for dirty/ignored/large/out-of-workspace files.
   Native file recovery cannot undo arbitrary remote mutations.
4. Name the supported gateway/local versions and the initial native model snapshots.
   Generic endpoint support is not a promise of every upstream feature.
5. Confirm archive/blob migration on representative sanitized source fixtures,
   and set replay eligibility for historical records missing native opaque fields.

Supporting details and reproducible RE are indexed in
[research/README.md](research/README.md); detailed execution design is in
[research/owned-engine-design.md](research/owned-engine-design.md).

## Validation snapshot for this review

Latest follow-up, 2026-09-29, against the uncommitted implementation described above:

- `cargo test --workspace --offline --locked`: **147 passed**, comprising 129
  shell tests and 18 engine tests. Headless and engine doc-test targets had zero
  tests. The previously failing seven API tests now pass; the harness installs
  the rustls ring provider before creating its reqwest client.
- `cargo clippy --workspace --offline --locked -- -D warnings`: **passed**.
- `bun test tests/engine-client.test.ts tests/native-events.test.ts tests/settings.test.ts`:
  **26 passed**, 266 assertions. This includes checking generated schemas against
  current OpenAPI; it does not exercise real UI hydration or native chat.
- `bun run typecheck`: **passed**.
- Earlier binary-inspection checks remain research evidence in
  [binary-evidence.md](research/binary-evidence.md); they do not certify engine
  implementation or task quality.

The old reqwest `No provider set` result was from the prior snapshot and is no
longer an open blocker. Remaining M0 gaps are the writer, migration coverage,
restart/resync, real client integration and baseline described above. No native
provider calls or installed-app UI smoke test were run for this follow-up. Only
this reviewed plan was amended; the original plan, checklist and ongoing runtime
implementation were left untouched.
