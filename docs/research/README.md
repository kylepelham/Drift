# Drift engine replacement research

The 2.0.2 research pass (2026-10-06) compared Drift at `4eb75f8023` with Claude Code 2.1.85,
Claude Desktop 2.19675 and its 2.26454 update. Five reports came out of it, listed first in the
table below. Two bugs it found are real and scheduled for 2.1.1: a stale whole-file `write` can
overwrite an outside edit, and imported skills' invocation flags are ignored. What Drift adopts is
decided in "After 2.0.2" in [engine-rewrite.md](../engine-rewrite.md) and in `CHECKLIST.md`; the
reports are evidence, not the plan.

The committed reports leave out bundle offsets and binary addresses. The full versions, the probes
and the IDA database are kept locally in `docs/research/private/`, which is not committed.

The following index records the original 2026-09-29 investigation and its
implementation-era checkpoints. For current implementation state, use
`CHECKLIST.md` and `docs/engine-rewrite.md`, not their historical status claims.

Original research snapshot: 2026-09-29. The goal was to replace the embedded OpenCode core
with Drift-owned execution and deep support for Anthropic, OpenAI and xAI, while
preserving subscription sign-ins, direct keys, gateways and local endpoints.
Research documents and reproducible inspection tools exist. No replacement engine
or provider login flow was implemented by the original investigation itself.

The plan of record is [engine-rewrite.md](../engine-rewrite.md), with implementation
state in `CHECKLIST.md`. It now includes the requested M3 async-worker additions.
The [separately reviewed plan](../engine-rewrite-reviewed.md) preserves the earlier
comparison and recommendations. The [owned-engine design](owned-engine-design.md)
supplies deeper design background under the selected Rust-library/shared-writer
architecture; newer canonical decisions take precedence.

For implementation status, use the [checkpoint at 896deaf4](checkpoint-896deaf.md),
following [fix verification at b99a6ec8](fix-review-b99a6ec.md) and
[the progress check at 772fe5c](progress-review-772fe5c.md). These checks separate
verified fixes from remaining recovery boundaries and deferred account/archive work.

For the immediate go/no-go decision, use [the M1 sign-off review](m1-signoff-review.md).
It limits blockers to the existing first-phase features and leaves later phases
with their own work, as requested by the user.

Current M2 progress is in [the b7cdf43 checkpoint](m2-checkpoint-b7cdf43.md): config,
skills, formatter and MCP foundations are implemented. Its
[db12943b follow-up](m2-checkpoint-b7cdf43.md#m2-follow-up-blockers-closed-at-db12943b)
verifies tool-set enforcement and late-connection fencing and clears the scoped
M2 code/automated review for moving to M3.

For current M3 product feedback, see
[conversations versus subagents](m3-conversations-and-subagents.md). It records the
user's branching intent, the persistent-subagent sidebar issue and proposed changes
without treating background workers as independent conversations.

The [9f4359f M3 checkpoint](m3-checkpoint-9f4359f.md) reproduces three bugs in
implemented work: failed compaction persistence loses request context, a turn
preparing credentials can write into the old workspace after a move, and a failed
subagent can return its compaction summary as its answer. Existing suites pass;
the report records the required regression coverage.
Its [4ca12e25b follow-up](m3-checkpoint-9f4359f.md#fix-verification-at-4ca12e25b)
verifies the three original fixes and reproduces one remaining worker-result
bug: child-only Stop during automatic compaction reports earlier progress as
a successful answer.
The [bcf770de4 verification](m3-checkpoint-9f4359f.md#cancellation-fix-verification-at-bcf770de4)
closes that cancellation finding. All reproduced findings in the checkpoint
are fixed; unchecked M3 work remains pending.

[Claude async-worker RE](claude-async-workers.md) now traces launch-mode selection,
registration, detached execution, notifications, cancellation and view lifetime in
the installed binary. It separates Agent arguments and skill/fork/workflow terms
from execution mode and supplies the pending M3 async contract added to the canonical
plan and checklist. Existing foreground and user-branch work remain implemented.

## Investigation results

The [independent audit at ac1ab72](independent-audit-ac1ab72.md) reproduces five
new failures in undo persistence, queue replacement, post-write capture,
worker mutation ordering and returned file mentions.

The [agent-loop follow-up at b93cbc9](agent-loop-review-b93cbc9.md) checks the
latest external review and records additional reproduced patch, UTF-8, worker
completion, attachment and error-body timeout failures.

The [worker follow-up at 06a5e0c](worker-review-06a5e0c.md) verifies the next
review's cancellation, delivery, queued-config and rollback findings, with
additional recovery, foreground-acknowledgment and replay-identity gaps.

The [delivery follow-up at f2830aa](worker-delivery-followup-f2830aa.md) confirms
the live retry gap, distinguishes Stop suppression from result attachment,
and checks foreground consumption and staged-replacement semantics.

The [staged recovery review at e92100f](staged-recovery-review-e92100f.md)
reproduces startup deletion of a stranded backup and an unread non-UTF-8 write,
and checks held results and staged-file inventory overhead.

The [M3 provider, question and MCP review at 83dacc0](m3-providers-questions-mcp-83dacc0.md)
reproduces stream-terminal, token, answer-race, stale-connection and snapshot
failures, and records the user's disable-versus-reconnect decision.

| Report | What it establishes |
| --- | --- |
| [Capability gating](claude-capability-gating-2.26454.md) | Current desktop/runtime source evidence for offer/prompt/discovery/execution gates and limits; small controllable capability groups, context/lifetime contracts and baseline-preserving rollout requirements for Drift. |
| [Host-native versatility](drift-versatility-2.0.2.md) | User direction and optional-worktree inheritance; traced computer/terminal contracts and verified stdin/media/effect limits; task coverage across apps, assets, games, training and RE without replacing existing CLI/MCP strengths. |
| [Claude Desktop 2.19675](claude-desktop-2.19675.md) | Actual installed desktop archive and IDA evidence for native VM boundaries; traced live-browser verification, lazy task worktrees, PR monitoring and side replies, excluding already-covered Drift capabilities and opaque remote review. |
| [Agent quality pass](claude-agent-quality-2.0.2.md) | Reproduced stale overwrite and missing evidence/verification feedback; measured unused schema and output bytes; prioritized correctness, completion evidence, task state and request reduction over blanket extra inference. |
| [2.0.2 gap pass](claude-gaps-2.0.2.md) | Reproduced ignored skill invocation metadata and wrong post-compaction context attribution; verified restore/discovery gaps and separated implemented work from optional Claude behavior. |
| [Binary evidence](binary-evidence.md) | PE layout, two identical readable source bundles, hashes, byte ranges, reproducible binary scan and full-source structural index. |
| [Execution](claude-execution.md) | Main agent loop, streaming and batch schedulers, hooks/permissions, aborts, retries/fallback, transcript persistence and subagent ownership. |
| [Async workers](claude-async-workers.md) | Full ordinary Agent and forked-skill routing, background lifecycle and version/gate limits; M3 job, result-delivery and cancellation contract. |
| [Context](claude-context.md) | Deferred discovery, cache construction, compaction and retry paths, file evidence, session notes and tool-result budgets. |
| [Tools and prompts](claude-tools-prompts.md) | Prompt assembly, native tool contracts, Windows behavior, file mutation/checkpoint boundaries, skills and verification controls. |
| [Native authentication](native-auth.md) | Claude binary OAuth/refresh, current Anthropic plugin, Codex and SuperGrok flows, native secret authority and route-specific acceptance cases. |
| [Provider contracts](provider-contracts.md) | Native Messages/Responses fidelity, capability profiles, gateway/local contracts, reasoning replay, caches, usage and conformance tests. |
| [OpenCode exit inventory](opencode-exit-inventory.md) | All 28 overlays, consumed SDK/routes/events, storage/build coupling, session migration and removal gates. |
| [Evaluation](engine-evaluation.md) | Deterministic fixtures, durability/failure drills, timing definitions, paired task-quality tests and release criteria. |
| [T3 Code adoption candidates](t3code-adoption.md) | Read-only source comparison covering capability profiles, skills, async questions, worker tests, scoped streams, Markdown, checkpoints and context references, with phase fit. |

## How deep the executable inspection went

The installed Claude Code 2.1.85 Windows executable is 237,718,176 bytes. Its
SHA-256 is `4a336dc188dc44289c801a33ba1868fa2b2b39d345e9a3316327218048c91669`.
The `.bun` section contains two identical 12,254,272-byte readable source runs.
The first source run parses as JavaScript with zero TypeScript-parser diagnostics.

That run contains 52,943 function-like bodies and 3,302,825 AST nodes, including
bundled dependencies. Structural indexing records function ranges, lexical parents,
syntactic calls, environment references and feature/telemetry call labels. It is
not a resolved call graph or a claim that every function was manually understood.

Eight focused investigations traced the relevant execution, context, provider,
authentication and migration mechanisms. Reports distinguish observed branches,
feature-gated code, proposed behavior, and unanswered runtime questions. Paid
inference, private credential files, account entitlements and remote feature flags
were not inspected. Static RE cannot tell us which branch an account actually runs
or how much faster a replacement will be.

## Strongest architectural conclusions

1. Preserve provider-native replay data. UI chat parts are too lossy to be the
   authority for signatures, encrypted reasoning, tool references and response IDs.
2. Own native auth as a host service. Plugin-local refresh promises and request
   string rewriting are not a durable subscription integration.
3. Treat admission, provider attempts and tool effects as separate durable states.
   A response fallback cannot undo a tool that already acted.
4. Schedule by resource and operation behavior. Parallelism belongs in runtime
   policy, not just the model's instructions.
5. Keep tool discovery local/selective and cache planning provider-specific. A
   smaller dynamic catalog can still lose on cache misses and extra model turns.
6. Move overlay invariants into owned services, especially config leases, asks,
   tree admission, bounded history and cancellation. Deleting patches is the final
   step, not the migration plan.
7. Measure task completion and recovery, not only first-token appearance. The
   current research identifies mechanisms and risks, not a speed or quality score.

## Reproduction tools

- `scripts/inspect-claude.ts`: read-only PE and printable-region scan, fingerprinted
  marker offsets, bounded probe and explicit temp-only extraction.
- `scripts/inspect-claude-source.ts`: parse a specified readable source range and
  write its structural index without executing the source.
- `tests/inspect-claude.test.ts` and `tests/inspect-claude-source.test.ts`: synthetic
  malformed-container, offset, chunk-boundary and AST tests. They contain no
  extracted Claude source.

See [binary evidence](binary-evidence.md) for exact commands and artifact ranges.
Raw source and full generated indexes remain in temporary local storage, not git.

## Implementation sequence

The [reviewed milestones](../engine-rewrite-reviewed.md#milestones) propose the
work order, with [migration details](owned-engine-design.md#migration-and-removal-gates).
The first deliverable should be a fixture-backed vertical slice:
durable input, a fake provider, native file/shell tools, approval, Stop, restart,
and a joined trace. Native auth fixtures for every required mode belong in that
slice. Real subscription conformance is mandatory before retiring those paths.

Existing live sessions and credentials require a one-writer cutover. Keep the old
engine as the behavioral baseline until the new engine passes the retained-feature
matrix. Do not test an experimental engine by submitting the same mutation to both.
