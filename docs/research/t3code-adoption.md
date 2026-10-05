# T3 Code: useful ideas for Drift

Inspected 2026-09-29. Official repository: <https://github.com/pingdotgg/t3code>,
linked from <https://t3.codes>. Shallow read-only clone: `examples/t3code`, ignored
by Drift's git configuration. Pinned commit:
`ff1db030b179ef712cacc0098366d976e2877f45`, release preparation for v0.0.44.
License: MIT, copyright 2026 T3 Tools Inc.

Drift comparison began at `abc102a380d8e9d65c309e930cf6b65b5e147d3f`, with
concurrent, unfinished config/skill changes present. Those changes were read,
not modified. Recommendations below fit later phases; they do not reopen M1.

## Main distinction

T3 Code primarily controls installed coding agents through adapters and exposes
them to desktop, web and mobile clients. Its README explicitly requires installed,
authenticated Claude/Codex/Grok/OpenCode runtimes. Some integrations manage their
own credentials or installations, but this is not a replacement native model/tool
loop of the kind Drift is building.

The strongest ideas are orchestration, state ownership, recovery, UI performance
and tests. Keep Drift's Rust library, native providers, shared SQLite writer,
Solid UI and no-tabs rule. Do not adopt the Electron/Node/Effect stack or depend on
external agent CLIs merely to obtain these behaviors.

## Ranked adoption list

| Priority | Adopt in Drift | Phase fit | Size of change |
| --- | --- | --- | --- |
| 1 | Reviewed model/route capability profiles separate from discovery metadata | M2 provider/config work | Moderate extension to the existing catalog and adapter contract |
| 2 | Separate user-invocable and model-invocable skill metadata | M2 skills | Small while the parser/UI contract is being introduced |
| 3 | Durable async questions with explicit answer/dismiss semantics | M2 async questions | Implement as part of the planned feature, not a new unrelated subsystem |
| 4 | Drainable background jobs and precise test milestones | M2 onward | Small primitive reused as background workers arrive |
| 5 | Compact progress projections, stable-ID coalescing and byte budgets | M2 performance measurement; M3/M4 delivery work | Start with output projection/budget, defer protocol changes |
| 6 | Connection health separate from projection freshness; scoped detail streams | M3/M4 remote and long-thread work | Incremental client/transport evolution |
| 7 | Incremental Markdown at correctness-safe boundaries | UI performance work alongside later phases | Benchmark first; port behavior to marked/Solid, not their parser dependencies |
| 8 | Checkpoint lifecycle separate from provider completion and rollback support | M3 revert/diff | Extend existing snapshots; retain Drift's safer dirty-workspace contract |
| 9 | Typed context references with separate attachment identity | Later composer/media work and M4 blob migration | Add when the richer context feature is scheduled |

These are adoption candidates, not all mandatory tasks before M2 can ship.

## 1. Capability profiles, not just model names

T3's manifest separates generic display/capability data from provider-owned adapter
configuration. It validates profile references, defaults and adapter payloads.
Invalid downloads retain usable metadata, refresh failures have a cooldown, and a
newer bundled edit can outrank an older disk cache by content revision.

Drift already has models.dev, a bundled snapshot and a tool profile, so keep those.
Add reviewed facts discovery cannot prove: supported reasoning controls, endpoint
protocol, tool/schema support, continuation/rollback, attachment modes and relevant
route limitations. Store their revision with a run. Fetch time is not the same as
metadata edit version. Missing support should make the UI unavailable with a reason,
not produce a silent compatibility stub.

T3's actual `ProviderAdapterCapabilities` distinguishes in-session model switching,
promptless continuation and conversation rollback. These are valuable distinctions
for Drift's later model-switch/revert work. Its profiles describe external agents;
translate them into native endpoint capabilities rather than reuse them verbatim.

Sources: `apps/server/src/provider/ModelManifest.ts:40-122,143-192`,
`ClaudeModelManifest.ts:6-49`, `Services/ProviderAdapter.ts:28-55,67-140`,
`docs/internals/model-manifest.md`. Compare Drift `llm/catalog.rs:26-72,94-119`.

Validation: synthetic model aliases, missing profiles, invalid adapter metadata,
old disk/new bundle, offline refresh and unsupported route-control combinations.

## 2. Skills distinguish who may invoke them

T3 keeps enabled state and user invocation permission distinct. A skill reserved
for explicit user invocation is still shown in the composer, while an agent-only
skill is not. Source/scope and workspace-specific catalogs remain separate from
display labels and deduplication.

The inspected Drift skill record has name, description and path. As M2 introduces
the parser, include `user_invocable`, `model_invocable`, enabled state, source and
scope if these conventional flags are supported. Enforce model eligibility in the
skill tool, not merely by hiding a menu row. Do not confuse a permission to use
subagents with permission to spawn visible sidebar threads.

Sources: `packages/client-runtime/src/providerSkills.ts:32-74,106-126`,
`docs/user/providers-claude.md:64-73`. Compare Drift `config/mod.rs:64-70`.

Validation: manual-only skill visible to user but refused for autonomous invocation,
agent-only skill absent from menu, disabled skill unavailable, workspace overrides.

## 3. Async clarification is a durable operation

T3's question path distinguishes a response delivered as a new message from a
blocking native callback. For an async answer it produces both the resolution and
the new user-message/turn intent in one command transaction. IDs derive from the
request so retrying is not a second answer. It looks up old pending questions from
durable storage rather than assuming the recent in-memory activity window is complete.
Dismissal without answering is allowed only for the async path.

Drift's current question service uses process-local oneshot waits. That is reasonable
for the blocking vertical slice. Use the T3 lifecycle distinctions when implementing
the already-planned M2 async feature: persist owner, request identity, answer and
follow-up admission together; permissions remain their own blocking protocol.
Keep native choice IDs distinct from display labels when the provider supplies them.

Sources: `apps/server/src/orchestration/decider.ts:1616-1817`,
`Layers/OrchestrationEngine.ts:238-250,273-327`,
`packages/client-runtime/src/pendingRequests.ts:12-45,123-187`.
Compare Drift `question.rs:39-87`.

Validation: answer after restart, request outside the recent window, duplicate answer,
lost acknowledgement, cancellation, dismissing async versus blocking input.

## 4. Workers with drain, tests with exact milestones

T3's `DrainableWorker` tracks outstanding queued and executing items. Enqueue and
the counter change atomically; completion decrements in a finalizer. `drain()` waits
for both queue and active work. Its test explicitly enqueues another item while the
first is running and verifies drain does not complete early.

The runtime receipt bus exposes checkpoint-baseline, diff-finalized and quiesced
milestones for tests. Its production implementation is a no-op; these signals do
not pretend to be durable command receipts or drive product correctness.

For Drift, introduce a small Tokio worker/drain abstraction only as MCP reload,
formatter, async-answer or snapshot workers need it. Tests can await channel/notify
milestones with deadlines rather than repeatedly polling or sleeping. Keep durable
submission receipts, external effect status and test-only signals separate.

Sources: `packages/shared/src/DrainableWorker.ts:40-69`, its test,
`apps/server/src/orchestration/Services/RuntimeReceiptBus.ts:23-62`.

## 5. Make UI progress cheaper without discarding evidence

T3 retains full tool payloads in persistence but projects compact UI activity.
It coalesces only repeated progress with a stable `(turn, toolCallId)` identity.
Anonymous calls and different turns are not merged. Completion, unrelated events
and synchronization markers flush pending updates immediately.

Its default progress window is 50 ms, maximum pending update count 512. A separate
live budget counts retained items and serialized bytes, including items waiting for
an RPC acknowledgement; defaults are 1,000 items and 8 MiB. Those are T3 policy
choices, not numbers proven right for Drift.

Drift can first separate UI previews from full tool-result payloads and add measured
byte bounds. Then coalesce replaceable progress at stable IDs. Never drop text
fragments, permission requests or terminal events, and do not add a pacing delay
to first visible text. Slow clients should resync rather than retain unlimited output.

Sources: `ThreadLiveEventCoalescer.ts:18-94,158-198`,
`LiveStreamBudget.ts:9-94,131-181`, `ActivityPayloadProjection.ts:164-205` and the
coalescer tests under `apps/server/src/orchestration/`.

Validation: parallel same-label calls, missing IDs, turn boundaries, completion during
progress, slow reader, oversized single payload and exact text reconstruction.

## 6. Separate transport connection from synchronized data

T3 has one connection retry owner per environment. Authentication/offline failures
wait for a meaningful wakeup rather than repeatedly retrying unchanged conditions.
Socket readiness, shell synchronization and thread detail freshness are separate.
State and its applied cursor are cached together; an obsolete owner cannot overwrite
the successor cache. Mutations are not automatically replayed by reconnect.

Thread detail is loaded in user-turn windows, subscribed only while needed, and
retained briefly for back navigation. Desktop adds keep-alive consumers for active
threads; it waits for detail to observe completion before releasing them. This avoids
making every thread carry every other thread's large transcript.

Drift already has a good small local socket implementation and M1 recovery fixes.
Extend it when remote/larger-session work arrives: one owner, typed offline/auth
states, explicit synchronization state, scoped detail demand and bounded retained
state. Reuse one socket by multiplexing logical subscriptions if that remains the
chosen API. The T3 implementation is a reference, not a reason to add React/Effect.

Sources: `docs/internals/connection-runtime.md:9-83`,
`packages/client-runtime/src/connection/supervisor.ts`,
`state/threads.ts:43-50,141-196` and `state/threadRetention.ts`.

## 7. Parse only the changing Markdown suffix where safe

T3 caches pristine parsed nodes through a closed top-level code fence followed by
a blank line. Later appends parse only the suffix and adjust positions. It falls
back to full parsing for document-wide definitions, CR/BOM cases, changed prefixes
and unknown plugin behavior. Cached nodes are cloned because transforms mutate them.

Drift already reconciles rendered DOM/code chunks, but the inspected Markdown memo
still calls `marked.parse` for the whole text on updates. A code-heavy long response
is a useful benchmark. An independently written stable-block/suffix parser could
reduce parsing work, but marked's grammar and Drift sanitization differ. Port the
correctness strategy, not the remark/unified dependencies. A visible DOM retained
between frames is not proof that parsing work was avoided.

Sources: `apps/web/src/markdown-incremental.ts:33-105` and its tests.
Compare Drift `src/ui/markdown.tsx:929-933`. Validation includes reference links,
footnotes, open fences, edits to earlier text, CRLF, sanitization and final-output
equivalence. No speed improvement has been measured here.

## 8. Checkpoint lifecycle is separate from turn completion

T3 checkpoints use isolated temporary indexes and hidden refs, with explicit object/ref
flush settings and cleanup of private index locks. It also distinguishes provider
completion from checkpoint/diff quiescence, so background filesystem work does not
inflate apparent model duration. Revert checks whether the provider can roll back
its conversation before changing the files.

Drift already has separate shadow Git directories and pre-write snapshots; keep
that working design. For M3 add explicit baseline/end references and checkpoint
status, coherent conversation/file reversal, failure recovery and independently
measured checkpoint cost. Consider temporary indexes for concurrent work and Git
durability settings. Do not copy T3's workspace-wide `git clean -fd`/staging restore
behavior as Drift's default: preserve unrelated dirty user work and declare scope.

Sources: `docs/internals/overview.md:72-92`,
`apps/server/src/vcs/GitVcsDriver.ts:766-805,1039-1125`,
`checkpointing/CheckpointStore.ts:25-97`, `CheckpointReactor.ts`.
Compare Drift `session/snapshot.rs:33-101`.

## 9. Structured context references and attachment claims

T3 separates a context record, an occurrence in prose and the server-owned attachment
binding. Labels never identify resources. Canonical references preserve where the
user placed a file, terminal excerpt or review comment. Provider projection includes
each referenced payload once, with bounded fields and escaped delimiters; original
message structure stays readable. This is serialization discipline, not a guarantee
against model prompt injection.

Attachment uploads begin pending, are claimed into a thread at admission and have
stale-upload cleanup. Drift should keep its intended content-addressed blobs rather
than copy T3's UUID/path naming. Typed context and stable bindings would make future
terminal/file/review chips safer to copy, retry and render on a remote client.
Add this when composer/blob work is scheduled, not as a prerequisite for ordinary M2.

Sources: `packages/contracts/src/composerContext.ts:19-57`,
`packages/shared/src/composerContextReferences.ts:26-106,133-149`,
`apps/server/src/attachmentStore.ts:24-26,171-240`,
`docs/internals/composer-context-references.md`.

## Useful architecture, without adopting the whole system

T3 serializes commands and commits event rows, read projections and accepted-command
receipts in one SQL transaction, then publishes. External work runs after durable
intent. This is useful evidence for Drift's later async jobs and lifecycle operations.
It does not require replacing the whole Rust store with event sourcing. A focused
outbox/operation receipt inside the existing single-writer transaction may be enough.

Do not weaken Drift's payload-hash conflict checks: the inspected T3 receipt check
guards command ID/aggregate identity, not an entire same-ID payload fingerprint.
Both systems' policies need their own explicit tests rather than assuming the other
implementation is automatically stronger.

Sources: `Layers/OrchestrationEngine.ts:144-171,273-341` and
`persistence/Layers/OrchestrationCommandReceipts.ts:18-66`.

## What I would not bring over

- External agent subprocess dependency, Electron, React, Effect service graph or their
  RPC framework. They solve a different product architecture.
- Tabs or worktree/browser panel conventions that conflict with Drift's sidebar-only
  thread model.
- Full T3 relay/mobile/provider installation machinery while Drift's local rewrite
  and existing companion are still the priorities.
- API marketing or user testimonials as performance evidence. Static source cannot
  establish task-quality or latency superiority.
- CLI-specific model/auth quirks as Drift's native API specification.

MIT permits code reuse with its notice, but the present rewrite is independently
authored. No T3 code was transplanted into Drift. Any future copied substantial code
must retain attribution and be reviewed separately; algorithms and test scenarios
can instead be implemented in Drift's own language and contracts.

## Immediate recommendation

For the M2 work already in progress, prioritize model capability profiles, skill
invocation metadata and the async-question lifecycle. Adopt drainable test workers
as the first background services land. Keep performance/remote/revert/context ideas
in their owning later work rather than expanding the M2 exit criteria wholesale.

The clone is unchanged and no T3 dependencies or processes were installed/run.
This is a source comparison with inspected tests, not a runtime validation of T3.
Drift runtime code, canonical plan and checklist were left untouched.

Verification on the concurrent Drift working tree: `bun run typecheck` passed.
`cargo test -p drift-engine --offline --locked` did not finish within 240 seconds;
the unfinished test was `session::turn::tests::workspace_config_shapes_the_turn`.
In the inspected test, `let request = &h.provider.requests.lock().unwrap()[0]`
retains the guard while a subsequent turn needs the same request log. This is a
likely test deadlock, not a finding about T3 adoption. The test and other active
M2 files were left unchanged. No complete engine-suite pass is claimed here.
