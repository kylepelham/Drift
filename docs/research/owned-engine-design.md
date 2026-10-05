# A Drift-owned agent engine

Status: research-backed design detail, 2026-09-29. The plan of record is
`../engine-rewrite.md`; the separate proposed revision is
`../engine-rewrite-reviewed.md`. This document elaborates that proposal within the
repo's Rust-library, single-database architecture. Research and inspection tooling
are implemented; observed concurrent engine scaffolding is not claimed as our work.
No performance win has been established.

## Product contract

Drift owns the model loop, provider transports, authentication integration, tool
execution, context projection, persistence, and extension interfaces. No OpenCode
runtime, SDK, plugin loader, generated prompt dependency, or build overlay remains
after cutover. Retain the desktop UI, workspaces, sidebar threads, remote companion,
and useful host services.

First-class provider families are Anthropic, OpenAI, and xAI. Access requirements
are fixed by the user: direct API keys, Claude subscriptions, ChatGPT/Codex
subscriptions, gateways, and local models, with native login rather than the
current Anthropic plugin. Existing SuperGrok support also belongs in the parity
inventory. Supporting an access mode means owning its auth and request behavior,
not silently substituting an API-key account when a subscription fails.

Native here means implemented by Drift's Rust core and platform services. It does
not mean every vendor API shares one JSON shape. Bun remains the frontend,
conformance and research-tooling runtime.

## What the research changes

| Evidence | Design consequence | Expected benefit, not yet measured |
| --- | --- | --- |
| Claude selectively discovers tools and retains discovery across compaction. Drift's Jev selects whole servers and can delay inference. | Stable per-tool registry and discovery state; optional provider-native deferred references. | Smaller useful prompts without a mandatory classifier request. |
| Claude has safe/unsafe scheduling, but streaming fallback can discard messages after a tool has acted. | Bounded resource scheduler plus provider-attempt and durable-effect identities. Mutations wait for a successful completed provider response. | Fewer shared-state races and duplicate mutations. |
| Claude refreshes OAuth under a cross-process lock; current plugins mostly single-flight inside one loader. | One host credential authority, generation fencing, atomic refresh persistence. | Reliable native subscription sessions across windows and sidecars. |
| Signed thinking, encrypted reasoning, tool references and continuation IDs differ across providers. | Preserve native replay blocks; normalize only UI and scheduling facts. | Avoid subtle reasoning/context loss caused by chat-shaped normalization. |
| Claude's file conflict checks can miss same-mtime changes; direct-write fallback weakens atomic replacement. | Content-version checks and locked atomic replacement with explicit failure. | Better preservation of concurrent user edits. |
| Drift's real chat path acknowledges `prompt_async` before durable admission. | Persist input before acknowledging; execution is separate. | No accepted-looking prompts lost during restart. |
| Twenty-eight overlays fix actual ownership and race problems. | Reimplement their invariants directly in owned services and fixtures. | Remove patch maintenance without deleting the behavior it protects. |
| Claude's first-chunk metric is not necessarily first visible text. | Trace admission, preparation, first event, first text, paint, tools and completion separately. | Optimize measured delays rather than the wrong proxy. |

Evidence and qualifications: [execution](claude-execution.md),
[context](claude-context.md), [tools and prompts](claude-tools-prompts.md),
[native auth](native-auth.md), [provider contracts](provider-contracts.md),
[exit inventory](opencode-exit-inventory.md), and
[evaluation](engine-evaluation.md).

## Runtime and ownership

Use `crates/drift-engine` linked into Tauri, as required by the existing plan.
`crates/drift-engined` is a headless launcher for the same library, not a production
sidecar. `crates/drift-migrate` owns one-time import. The dominant suspected delays
are network, model work, context volume and tool execution. Rust and removal of
engine IPC establish ownership and simplify deployment; neither alone proves a
latency improvement. Benchmark those claims.

Use Tokio task supervision, typed cancellation, serde boundary types, axum HTTP/WS
and a single writer. Reuse existing Rust dependencies where appropriate. Do not
rebuild OpenCode's dependency injection or maintain legacy and V2 runners in the
new core. Generate TypeScript DTOs/client from the engine's OpenAPI.

```text
Solid desktop / remote UI
           |
   Drift client + owned DTOs
           |
  Tauri local gateway and remote TLS/device authorization
           |
   Drift core library in the Tauri process
     admission -> per-session runner -> context/prompt planner
                       |                       |
                  tool scheduler        provider adapter
                       |                       |
              built-ins / MCP          native SSE / later WS
                       |
               transaction writer + outbox

  Tauri host services
     credential authority / quotas / process supervision
     workspaces / archive metadata / previews / voice / updates
```

Shell and engine share one `drift.db` and one connection owner/serialized writer.
Two separately mutexed SQLite connections are not this design. Move existing shell
store operations behind the shared writer before linking engine migrations into
startup. Search and maintenance use owned queries, not guessed `opencode.db` paths.
Keep the current Tauri-resolved database path during upgrade; any data-directory
move is a separate migration.

Host/core database changes can share a transaction because they share a writer.
External files, credential vault writes, legacy databases and remote effects still
need recoverable operation IDs, ownership epochs, phases and acknowledgements.
Desktop, remote and background commands all pass through the same admission checks.
Never infer that an unacknowledged operation did not commit.

For purge, core atomically tombstones the session against new admission before
deleting data and scheduling blob collection. Host clears its archive tombstone
only after confirmed deletion. Restore must cancel an uncommitted purge or return
a conflict after deletion admission; it cannot report success while core deletion
continues. MCP approval publication similarly carries its exact effective-config
fingerprint and revision, with core acknowledgement before reporting it applied.

Credential storage and OAuth refresh have one Rust auth authority in `llm/auth`,
with a platform vault backend supplied by the host or headless launcher. Providers
obtain in-memory access credentials for pinned accounts/routes; UI and remote
clients never receive them. A credential lease constrains Drift's use of an access
token, not the issuer's validity period. The headless binary reuses this service.

## Proposed module boundaries

These are implementation destinations, not files to create as empty placeholders.

```text
crates/drift-engine/src/
  api/            axum HTTP/WS, owned commands/events, OpenAPI
  store/          SQLite migrations, writer, blobs, imports, retention
  session/        admission, runner, jobs, context, forks, moves, revert
  llm/
    prompt/       owned sections, model recipes, instruction provenance
    anthropic/    Messages encoder/decoder and route profiles
    openai/       Responses encoder/decoder and Codex route profile
    xai/          Responses encoder/decoder and SuperGrok route profile
    compatible/   explicit Chat/gateway/local endpoint profiles
  tool/           registry, schema lowering, scheduling, built-ins
  edit/           exact replacement, apply_patch, file versions/formatters
  mcp/            approval, transports, OAuth resource client, supervisor
  config/         immutable generations, skills, agents, commands
  permission/ question/ hook/ platform/
```

Provider modules import contracts and pure request data, never UI or workspace
mutation services. Tool implementations cannot call model providers directly.
Agent spawning submits a child job through session admission. Context preparation
is a pure projection where possible, with explicit reads for required artifacts.
Config reload publishes a new generation; it does not dispose an active runner.

## Three histories, each with one purpose

1. The durable domain journal records admitted inputs, provider attempts, tool
   execution states, asks, config revisions, compaction boundaries and terminal
   outcomes. It is the authority for recovery.
2. Provider replay records retain ordered native blocks/items and required opaque
   fields. They are the authority for the next same-provider request.
3. UI projections contain readable messages, tool cards, progress and estimates.
   They are not fed back to reconstruct signed or encrypted provider history.

These can share tables and blobs without sharing meaning. Do not persist the same
full transcript as an event, a JSON snapshot, and a provider request on every
token. Store immutable blocks once and reference their identity from projections.
Batch text checkpoints and make their durability explicit. Admission, tool intent,
tool terminal outcomes, accepted answers and compaction publication are transactional
boundaries; streaming animation is not an fsync-per-token requirement.

All WebSocket events carry monotonic `seq`. Distinguish durable outbox events from
attempt-scoped provisional deltas in their payload. The publication sequencer
must prevent sequence reuse across restart, for example through persisted range
reservation; monotonic need not mean gapless. A ring cursor only resumes events
still retained. Missing deltas, restart or an expired cursor produce `resync` and
an authoritative revisioned snapshot. Do not promise durable replay of provisional
text. Commit/invalidation resolves the attempt's projection; after restart show
the last persisted text checkpoint as interrupted. Bound subscriber buffers.

Proposed minimum records:

| Record | Identity and essential fields |
| --- | --- |
| Session | stable ID, workspace/location, parent/job links, active generation, imported provenance |
| Input | client submission ID, session, payload hash, steer/queue mode, admitted sequence, status |
| Turn | input IDs, run/cancel generation, agent/model/profile/config revision, terminal reason |
| Provider attempt | attempt ID, turn/step, request fingerprint, capability revision, sent/completed/error state, native response identity, usage |
| Provider block | attempt, ordinal, block/item/call IDs, typed kind, native replay blob, UI projection, replay eligibility |
| Tool execution | execution ID, provider call binding, effective argument hash, permission receipt, resource locks, effect state, result reference |
| Ask | stable request ID, owning session/turn, permission or clarification, answer hash, pending/resolved/expired state |
| Context revision | source cursor, retained blocks, omission reasons, summary provenance, discovered schemas, token estimates |
| Outbox event | global sequence, session, entity revision, versioned payload, commit association |
| Import ledger | source database identity, source row mapping, checkpoint, content hash, committed import revision |

Keep provider call IDs, local execution IDs, and remote idempotency keys separate.
Two different calls with identical arguments may be intentional. Do not deduplicate
them by arguments alone. Map a remote idempotency key to the durable execution only
when that tool explicitly supports it.

## Admission and the session runner

`SubmitInput` carries a client-generated submission ID and an immutable payload.
The core inserts it and an outbox event in one transaction, then returns the
admitted sequence. An exact retry returns the same receipt. The same ID with a
different payload or delivery mode is a conflict.

```text
admitted input
   -> runner owns session generation
   -> promote eligible steer/queue inputs
   -> pin account, capabilities and config
   -> project context and tools
   -> prepare and send provider attempt
   -> decode provisional events
   -> commit terminal provider response or failed attempt
   -> tools / clarification / compaction / completion
   -> next provider step or terminal turn
```

Only one runner drains a session at a time. Different sessions can run concurrently,
subject to shared tool/resource and account limits. Session-tree moves and forks
have a coordinator that excludes new admissions during their atomic cutoff. Do
not implement move safety as a frontend busy check.

Pin the effective config, account/route, tool-definition set, capability profile and
agent recipe for the whole run, including all its provider steps. Tool discovery
can select among that pinned set; it does not refresh definitions mid-run. A deliberate
retry-model switch records a transition, compatibility decision and new lineage
before rebuilding context. Catalog/config refresh affects later runs. A hard
permission revocation can block dispatch or cancel a run without silently swapping
its definitions or account.

Steering enters at the next safe provider-step boundary. Queued input waits for
the current turn to settle. Async clarification replies become durable inputs
with the original owner/model/context; permission replies authorize only their
specific pending execution. All wake-ups are advisory because pending work is
already durable. A process restart scans state and exposes recovery choices; it
does not automatically restart every generation that might already be billed.

Cancellation increments a generation before stopping children. Late completions
may record a result for an execution that already happened, but cannot start new
tools, revive the turn, or satisfy an ask in a later generation. Repeated Stop is
idempotent. Record user cancellation, transport failure, timeout, supersession,
permission denial and hook termination as different outcomes.

Expose `stop_requested`, per-child acknowledgement or timeout, and final quiescence
separately. A remote request already sent may still incur charges, and a command
already dispatched may still finish or leave an unknown effect. Generation fencing
stops new dispatch and stale continuation; it is not a remote rollback mechanism.

## Provider-native adapters

Use one provider contract with separate native implementations. An illustrative
interface:

```rust
trait Provider {
    fn validate(&self, input: &PreparedTurn) -> ValidationResult;
    fn encode(&self, input: &PreparedTurn) -> EncodedRequest;
    fn stream(&self, request: EncodedRequest, cancel: Cancellation) -> ProviderStream;
    fn project_replay(&self, history: &ReplayHistory, profile: &CapabilityProfile) -> ReplayPlan;
    fn classify_failure(&self, error: &TransportFailure, progress: &AttemptProgress) -> RetryDecision;
}
```

The native replay block is a tagged value keyed by provider and protocol version.
Unknown fields are retained when safe to store, but never executed or promoted
into tools simply because they appeared in an event. Provider usage remains
available in its original form alongside normalized counters. Missing usage is
unknown, not zero.

| Family | Required fidelity | Distinct access routes |
| --- | --- | --- |
| Anthropic | content-block order, thinking signatures/redacted thinking, tool references, cache controls, server tool ownership, stop/pause/compaction reasons | API key, Claude subscription, Console profile, native Messages gateways |
| OpenAI | response/item/call IDs, message phase, encrypted reasoning, output ordering, explicit continuation lineage, incomplete/failed states | API Responses, Codex subscription backend, compatible Responses gateways |
| xAI | encrypted reasoning where supported, own Responses restrictions, tool/citation/usage fields, sticky cache routing, distinct chat behavior | xAI API, verified SuperGrok route, supported gateways |
| Local/compatible | actual served model/context/modalities, declared tool format, tested SSE/Chat/Responses behavior, missing capability diagnostics | LM Studio, Ollama, explicitly configured compatible servers |

This table summarizes the [full protocol and route contracts](provider-contracts.md).
Features such as server compaction, WebSocket continuation and hosted tool search
must be profile-gated. A field accepted but ignored by a server is not supported.

Pin capabilities by provider, endpoint, model snapshot, auth mode and revision.
Keep a small reviewed native model catalog, augment with runtime discovery, and
allow explicit user endpoint profiles. Do not infer protocol fidelity from an
`openai-compatible` label or select model behavior by substring matching alone.
Model aliases can advance; preserve the profile used for each saved attempt.

Account changes or incompatible model/route switches start a new continuation
lineage. Never send another account's response ID, thinking signature, ciphertext,
or private remote file handle to the new route. Cross-provider continuation is a
declared lossy export of supported content, with original history retained locally.
The same rule applies to a model switch within one provider when compatibility is
unverified. Record omitted material and unresolved call/result pairs explicitly;
a visible reasoning summary cannot recreate opaque reasoning state.

### Native subscription login

Ship owned host auth methods for browser PKCE, provider-specific device flow,
API-key entry, and local/gateway profiles. Each login attempt has state, expiry,
redirect ownership and one-use completion. Each account record has a generation.
Refresh is single-flight across processes, re-reads under ownership, atomically
persists rotated credentials, and fences logout/account switches.

The [auth report](native-auth.md) recovers actual compiled OAuth endpoints, scope
sets, refresh-lock behavior, and current plugin transformations. Native Claude
support must not be defined as copying the plugin's synthetic identity/billing
strings and hoping they continue working. Native ownership can remove npm and
OpenCode dependencies; it cannot create an issuer-supported third-party contract.
Keep that compatibility uncertainty visible and test the approved subscription
route independently. Never silently charge an API key when it fails.

All required auth profiles get fake-issuer and request fixtures from the first
vertical slice. Live login/entitlement checks are a cutover gate for each real
route; this investigation performed none. Direct-key implementation progress is
not permission to drop subscription support from the replacement.
An unverified subscription route blocks replacement of that access mode. Gateway
and local conformance applies to named, versioned endpoint profiles, not every
server that happens to accept a similarly shaped HTTP request.

## Tool registry, discovery and execution

The registry stores a stable tool ID, version, description/hint, input schema,
output contract, execution owner, permission category and scheduling policy.
Model-facing names map reversibly to stable IDs. Schema lowering must detect
collisions and retain canonical validation even when a provider needs a narrower
schema dialect. MCP annotations inform policy; local policy can override them.

Discovery is local exact-name/prefix/keyword search first. Its output identifies
selected tools and schema versions. Persist discovered-tool state with the
context revision so compaction and resume do not force unnecessary rediscovery.
On Anthropic, use native deferred references when supported. On other routes,
use their documented discovery protocol or an explicit next-request schema
update. These fallback updates may break prompt caching and must be measured.

The selected plan drops Jev routing and the `execute` code-mode tool. Preserve
their current behavior in baseline measurements so local-model regressions are
visible. The new default is direct tools plus selective discovery. Reintroducing
code mode would be a separate evaluated feature, not an implicit JS plugin host.

### Scheduling policy

Classify operations as verified read-only, idempotent mutation, mutation, or
unknown. Attach resource claims such as workspace/path, desktop target, browser
session, database or remote account. A shared-state read can require an exclusive
resource claim even though it does not change business data. Default unknown
tools conservatively; unrelated tools should not share a global lock forever.

Use a bounded FIFO admission queue with deterministic ordering, atomic acquisition
of each operation's resource set, and a documented fairness policy. Cap both
streaming and batch dispatch. Avoid nested lock acquisition inside tool code.
Stop granting new work after cancellation. Store permission outcome and the exact
post-hook, revalidated arguments before dispatch.

Verified read-only work may run after a complete validated tool-call block arrives,
before the overall provider response finishes, if that adapter guarantees stable
block identity. Its output is provisional and discarded with the failed attempt.
Unknown or mutating work waits for a successful terminal provider response. This
is a deliberate correctness/latency tradeoff to benchmark against Claude's early
dispatch. No partial JSON, ambiguous tool name or streaming fragment can execute.

Speculation requires an explicitly certified non-effectful implementation, not just
an MCP read-only hint. It must not navigate, consume remote state, charge a tool
account, run effectful hooks or open a permission prompt. Work that needs approval
waits for response commit. Assign speculative execution IDs separate from durable
effect IDs; promote a result only after the terminal response contains that exact
validated call with unchanged arguments. Discarded read evidence cannot authorize
a later write. Never send the next provider request before resolving speculation.

Record intent before invoking a mutating tool and its outcome before the next
provider request. If the process dies between a remote mutation and recording the
result, mark `outcome_unknown`. Query an operation receipt or use a supported
idempotency key before retrying. Otherwise ask the user or require explicit
reconciliation. A local database cannot provide universal exactly-once execution
of arbitrary external tools.

### Files, shell and checkpoints

Read evidence includes canonical path/file identity, full or partial coverage,
content digest, encoding and line endings. File mutations lock the resolved
resource, verify content identity, generate the patch, and use atomic replacement
where supported. If replacement fails, report failure; do not silently fall back
to a partial direct write. An outside writer can still race between check and
replacement on filesystems without a conditional replacement operation. Do not
promise a cross-process compare-and-swap that the OS does not provide.

Preserve encoding and line endings by default. Patch tools may safely operate on
validated ranges without requiring every large file in model context; whole-file
overwrite needs whole-file version evidence. A grep hit is not read evidence.
Track the old and new content addresses for revert, including deletion/creation.
Shell changes need explicit workspace-diff/checkpoint handling and cannot be
assumed covered by native Edit history.

Use actual platform shell contracts. On this host that includes PowerShell, its
quoting rules, working directory semantics, cancellation and process-tree cleanup.
Do not reuse Bash instructions merely because the tool was named `bash` upstream.
Return typed truncation, stable retrieval handles, cursor/revision and exit/timeout
metadata. Count command failure, kill requested, and kill confirmed separately.

## Context, prompts and compaction

Build context from durable replay blocks with explicit inclusion reasons and
budgets: owned system sections, user/workspace instructions, tool schemas,
conversation, media, tool outputs, generated notes, and recent evidence. Put stable
material before volatile facts where the provider's cache contract permits it.
Record request and tool-catalog fingerprints plus actual provider cache usage.

The UI consumes this prepared-request breakdown. It no longer guesses system/tool
usage by subtracting an incomplete transcript estimate from the reported total.
Preflight estimates and actual post-response usage are shown as different data.

Compaction creates a new request-view revision, never destroys original evidence.
Persist its source cursor, retained tail, unresolved tasks/asks, discovered tools,
summary and omitted artifact handles atomically. Select whole valid tool-use/result
pairs. A failed, canceled or oversized summary leaves the prior revision active.
Use bounded attempts and a failure circuit breaker. Restore selected fresh file
evidence where it improves continuation; record its cost and revision.

Generated memory is optional and separate from user instructions. Store provenance,
source cursor and staleness. Do not enable background note-extraction model calls
by default merely because Claude has the feature behind a flag. Test whether its
extra calls improve long-task correctness and total cost.

Prompts are owned sections with provenance, rendered by explicit model recipes.
Generate tool instructions from the same contracts that execute tools. Preserve
instruction authority and keep retrieved files/tool text identified as data.
Natural-language conflict resolution and prompt-injection resistance cannot be
guaranteed by a prompt compiler; hard permission/resource constraints remain
runtime checks. No copied Claude identity, vendor marketing, or exact-prefix
substitution of upstream prompts.

## Extensions and runtime configuration

Config generations own providers, tool schemas, MCP clients, skills, commands,
agents, formatters and language services. Each running turn and detached child
leases its generation. New idle turns take the latest revision; old resources
close only after the final lease. Approval applies to the final effective MCP
definition before connection, including dynamically added definitions.

Keep MCP interoperability and conventional skill/agent files through explicit
importers or owned parsers. Native Drift UI plugins remain separate. Engine hooks
are an internal Rust `Hook` trait with typed inputs/outputs, with no JavaScript
plugin host. Arbitrary OpenCode plugins are imported as unsupported config entries,
not executed. Provide documented replacements for the product behavior we retain.
Hook input/output has a schema, deadline and authority. Revalidate changed tool
arguments and run the final permission decision after pre-execution hooks.

Subagents and spawned sibling threads are durable jobs with explicit inheritance:
context cutoff, model/effort, permissions, tools, budget, account and cancel policy.
Receipts and parent links live in storage, not just UI localStorage. UI-driven
autonomous continuation moves into the runner. Closing a window must not lose its
round count, pending clarification or Stop state.

## Migration and removal gates

The [exit inventory](opencode-exit-inventory.md) maps all 28 overlays and the actual
legacy/V2 dependencies. Use that ledger as a parity contract, not the upstream
directory layout as the blueprint for the new core.

1. Capture deterministic provider/tool and current UI protocol fixtures. Add
   joined trace records to the baseline. Freeze correctness criteria before tuning.
2. Define owned DTOs, storage migrations and credential authority. Build a sandboxed
   vertical slice through durable admission, fake provider, read/edit/shell,
   permissions, cancellation and restart. Exercise fake auth for every access mode.
3. Implement native provider routes and owned prompts. Preserve lossless replay,
   profile-specific errors and usage. Run account-isolated live conformance only
   when authorized; do not use live traffic as the unit-test runner.
4. Port config leases, MCP approval/reconnect, scheduling, asks, agents/skills,
   compaction, fork/revert/move and local/code-mode semantics. Check off every
   overlay invariant, including patches that become unnecessary workarounds.
5. Use a temporary boundary translator for the UI routes/events actually consumed.
   It translates protocol only and has no runner, auth or model logic. Old/new
   execution selection is fixed per session during migration; never double-submit
   model or tool work as a shadow test.
6. Import legacy sessions from WAL-consistent shared and channel database snapshots
   into the shared `drift.db` with stable IDs and a transactional import ledger. Preserve
   unknown raw historical parts and archive joins. Historical tool calls remain
   history, not instructions to rerun. Imported signed replay is eligible only
   after provider/profile compatibility is established.
7. Quiesce old engine, watchers, quota writers, maintenance and remote writes;
   perform final import, commit the host authority epoch and retain rollback evidence.
   Rollback must account for new-core sessions and rotated credentials, not simply
   reopen the stale old database and lose new work.
8. Remove translator after the UI uses owned DTOs. Remove SDK imports, npm auth
   plugins, overlay tooling, generated upstream prompts, direct OpenCode database
   readers, vendored build dependency and unused notices. A clean offline build
  must not need `engine/upstream` or an OpenCode plugin install.

Persist cutover phases in the host before each external action:
`prepare -> ingress_fenced -> old_owner_stopped -> snapshot_sealed -> imported ->
new_owner_selected -> verified`. Every phase has restart recovery and an operation
receipt. The authority-row transaction selects the linked core as writer; it does
not make legacy file copies and credential rotation atomic. Refuse to activate a
headless or legacy engine with stale ownership. Before writer selection, rollback can discard the staged
import. After new writes or credential rotation, rollback needs forward export or
an explicit recovery mode preserving new data and current credentials.

Imported sessions initially have render/export support and unverified continuation.
Validate attachments, context boundaries and native replay eligibility before
allowing continuation. Do not convert a historical incomplete tool call into a
pending execution just because it lacks a result.

Retain attributed, licensed utility code only after explicit review. The proposed
core design is independently authored from behavior and protocol requirements;
the proprietary Claude source stays outside the repository. A fork that imports
OpenCode code permanently should state which licensed code it retains rather than
claiming it was written from scratch.

## What better must mean

The [evaluation plan](engine-evaluation.md) defines fixture formats, trace fields,
paired task methodology and failure injection. Before cutover, require:

- No acknowledged input lost in admission/crash tests.
- No automatic replay of an uncertain external mutation.
- No unauthorized tool dispatch or account/route credential crossover.
- Exact preservation of provider replay-critical fields in same-route fixtures.
- Correct stream recovery, compaction, archive/import and every carried overlay
  invariant across all required access profiles.
- Measured task correctness and latency at matched models, effort and budgets.
  A faster incorrect patch is a regression. Report cold/warm/cache strata and
  uncertainty; do not turn synthetic rendering results into model-speed claims.

The first implementation milestone is a recoverable vertical slice with fake
providers and tools, not a new settings page and not a replacement production
binary. The first performance milestone is a baseline trace and paired task run.
Those give the rewrite an observable contract instead of another pile of patches.
