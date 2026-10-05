# M1 sign-off review

Scope agreed with the user: review the first usable Anthropic vertical slice so
later phases can proceed. This replaces the broader readiness framing of the
[896deaf checkpoint](checkpoint-896deaf.md), not its verified observations.
Inspected `896deaf4cf048d22fac5ef00e4ea2388f2f7ec34` on 2026-09-29.

## Follow-up at a0fce77d

Verified `a0fce77d282033c606e525dea3d3984a2615c2b2` after the M1 correction and
conformance commits. The correction list below is now historical, not an open
request to implement it again.

| M1 correction | Follow-up result |
| --- | --- |
| Resolved file permissions | Paths resolve before read/search asks and read evidence. Traversal regression passes; the symlink test runs when the host can create the link. |
| Snapshot and persistence refusal | Snapshot failure prevents writing; a call that cannot record its start does not run; terminal/outcome save failure is surfaced as error. Fault tests pass. |
| Shell descendants on Stop | Windows job/unix process-group support exists. Tests for explicit abort and dropping the run future both prevent delayed descendant writes. |
| Replay and UI recovery | Malformed arguments fail before dispatch; MaxTokens dispatches nothing; unsigned reasoning/unparsed calls are excluded. Resync invalidates cached transcript loading, and the selected view refetches on returning online. Old-instance/disposed socket buffers are fenced. |
| Credential-write fence | Set, remove and conditional replacement now share the mutation lock. Concurrent replacement test passes. |
| Recorded conformance | Six black-box tests build/spawn the real headless engine and exercise recorded Anthropic SSE through HTTP/WS, signed thinking/tool replay, denial, retries, submission conflicts, abort, reconnect and restart. All pass. |

Current checks:

- Rust workspace tests: **264 passed**, 129 shell plus 135 engine; headless and
  doc-test targets have zero tests.
- Full Bun suite: **1,186 passed**, 8,422 assertions, including conformance.
- Standalone recorded conformance run: **6 passed**, 42 assertions.
- Application typecheck and workspace/all-target clippy with warnings denied: passed.

**M1 code/automated review is cleared within the agreed scope.** The explicitly
pending desktop smoke test is still needed for installed-shell UI confirmation:
open the dev shell, send a turn, approve/deny a tool, Stop a running shell, switch
threads and reopen a saved transcript. No native-window inspection was performed
in this follow-up. Later-phase items below are not additional M1 blockers.

This follow-up was performed directly. No delegated agents or visible sibling
threads were used after the user's delegation correction. `AGENTS.md` now makes
that restriction explicit for future delegated work.

## What belongs to M1

Native Anthropic key/subscription access, streaming/tool assembly, the turn loop,
read/edit/write/shell/glob/grep, existing permission and snapshot guarantees,
persistence, basic UI recovery and recorded conformance. Account profiles, MCP,
subagents, compaction, full lifecycle/migration and advanced shared-resource
scheduling are later work, not conditions for M1 approval.

## Passed foundations

- Shared writer, linked engine and native API/UI request path.
- Exact-match editing, read-before-write gates, permission round trip and successful
  snapshot path, all with current tests.
- Atomic prompt admission and durable sequential/restart submission deduplication.
- Payload mismatch rejection, tied pagination and earlier-page running status,
  verified again through isolated probes.
- Hydration load failures now reject instead of falsely acknowledging recovery.
- Tool runs preserve mutation barriers rather than moving reads ahead of writes.

## Earlier M1 correction list

1. **Permission checks must describe the file actually accessed.**
   `tool/mod.rs:58-64` joins paths without resolving `..` or symlink targets;
   `read.rs:30-35` then uses lexical containment to skip asks. `ws/../outside.txt`
   still starts with `ws` before filesystem resolution. Grep/glob always return no
   ask, even for explicitly outside paths. Resolve targets consistently before
   permission decisions and execution; test sibling traversal and symlink escapes.
   This is correctness of current permissions, not a future sandbox feature.

2. **Do not silently waive snapshot or persistence failure before a write.**
   `session/turn.rs:391-399` turns snapshot failure into `None` and proceeds.
   `finish`, `start_call` and `settle` also ignore save errors. Return a failed tool
   outcome before mutation when its required snapshot/intent cannot be recorded;
   do not publish successful terminal state after its save fails. Inject snapshot
   directory failure and result/intention write failures in the current harness.
   Full revert/dirty-tree redesign remains M3.

3. **Stop must account for shell descendants.**
   `tool/bash.rs:111-137` kills the direct shell, not a process group/job tree. The
   runner can drop the future on cancellation. A delayed child can still write
   after the session appears idle. Test a child doing a delayed write after Stop,
   and either terminate/reap the tree or expose incomplete cleanup honestly.
   New shell timeout preferences remain later-phase work.

4. **Complete the current replay and UI recovery boundary.**
   Aborted assistant rows are replayed wholesale, including incomplete signed/tool
   blocks. MaxTokens is checked after tool dispatch; malformed JSON becomes a
   string rather than failing call validation. Decide replay eligibility by valid
   completed blocks and reject incomplete call input before dispatch. The missing
   Anthropic final-marker case is still observable in the probe; document/test the
   accepted terminal contract rather than leaving it implicit. On UI resync, reload
   or invalidate already-loaded transcripts; current hydration only reloads lists,
   status and asks, and `openSession` skips cached messages.

5. **Finish the narrow auth fence, not the account-profile feature.**
   `Credentials::replace_if` takes `write_lock`, but `set` and `remove` do not.
   Make existing mutations participate in the same ownership so a login/logout
   cannot race the comparison and write. Add a concurrent replacement test.
   Multi-account storage, profile selection and its UI stay in M3 as documented.

The existing socket also needs instance/disposal fencing during outstanding
hydration, so old held events cannot apply to a new connection. Cover that alongside
item 4, not as a separate transport rewrite.

## Earlier final M1 gate

Complete the currently unchecked recorded Anthropic conformance suite. It should
exercise the actual adapter, HTTP/WS/UI recovery boundary and the failure cases
above, not only scripted `Chunk` values. Existing fixture/unit coverage is useful
but does not yet fulfill that whole criterion. The checklist's installed-shell run
is still pending and should be verified separately.

Before advancing, require the current M1 tests plus those targeted regressions to
pass. Do not require all later architecture proposals to be implemented first.

## Follow-ups that are not M1 blockers in this review

- Full account profiles and multiple credentials per provider.
- Consolidating shell/native archive representations and full retention lifecycle.
- MCP clients/approval, shared desktop/browser resource scheduling and subagents.
- Compaction, retry-model switching, migration and removal of the legacy dependency.
- Broad provider quality/performance comparisons and a universal execution journal.

Concurrent submission retry semantics, bounded read batches and purge/drain races
remain recorded follow-ups. They should get regression coverage with their owning
features; this review does not expand M1 into a lifecycle cutover.

## Checks run

- `cargo test --workspace --offline --locked`: 256 passed.
- `cargo clippy --workspace --all-targets --offline --locked -- -D warnings`: passed.
- `bun run typecheck`: passed.
- `bun test tests`: 1,178 passed, 8,375 assertions.
- Isolated probes reconfirmed the fixed admission/idempotency/paging/status cases.
  No live model, private credential or keychain access. No runtime source changed.

Assessment: the implementation is close enough to finish M1 without another
architecture overhaul. Close the specific current-feature correctness gaps and
its conformance gate, then move on to M2.
