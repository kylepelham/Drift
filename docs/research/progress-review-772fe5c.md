# Rewrite progress check at 772fe5c

Checked 2026-09-29 against `772fe5c21fb28fb3a62ebe9548b13f358642a297`, comparing
committed work with the previous `24ca7164ee5519fbb98138eda16452213b3c597c` base.
This is a read-only runtime review with local synthetic probes, not a new engine
implementation or a production-provider test.

Superseded for current status by [fix verification at b99a6ec8](fix-review-b99a6ec.md).
The original F1/F2 reproductions are now fixed; this document preserves the earlier
evidence rather than describing those faults as still open.

At the final check HEAD had advanced to `c5706af28`, with additional CORS work
uncommitted. Intervening commits changed documentation and development-shell setup,
not the core paths behind F1-F4. Commit `d04a59054` corrected the standards finding
below and marked native UI wiring done while explicitly leaving its shell run
pending. Verification counts below belong to the tested snapshot, not later edits.

## Assessment

The feature work is advancing quickly and the selected architecture is being
implemented. The shared database connection is fixed. Native Anthropic streaming,
subscription auth, tools, permissions, snapshots and a turn loop exist, and the UI
now sends native requests through a temporary adapter to its existing store types.
The earlier M0-only snapshot is obsolete.

The hard-to-retrofit execution and recovery contracts are still incomplete. Pause
provider breadth long enough to fix admission atomicity, terminal stream validation,
replay eligibility and durable tool outcomes. Passing happy-path tests does not
establish these properties; two local fault probes failed below.

## Verified improvements

- Shell store borrows the engine-owned connection instead of opening its own.
- WS hello includes an instance ID; future cursors resync; asynchronous hydration
  buffers events. Failure and overlap handling remain incomplete.
- Anthropic signatures and redacted thinking have a decode/store/re-encode path;
  tool-use and result IDs also survive the known-block conversion.
- Tool calls execute sequentially after the stream iterator finishes, reducing
  within-turn state races. Iterator EOF is currently mistaken for successful completion.
- Cumulative provider usage is merged rather than repeatedly added.
- Native API, keychain/file-fixture credential backend, OAuth PKCE/refresh, permission
  replies and frontend integration exist. The checklist still correctly leaves
  UI completion and recorded conformance open.

## Foundational findings

### F1. Missing terminal event permits execution

`llm/anthropic/mod.rs:139-152` ignores `message_stop`. `session/turn.rs:266-287`
returns success on EOF without requiring a terminal signal or stop reason.
`step` then marks the message done and dispatches assembled calls.

A local fake Messages endpoint emitted a complete read-tool block, then ended the
body without `message_delta` or `message_stop`. The engine reported the assistant
and tool as `done`, read a temporary fixture file and made another provider request.
No real model or credential was used.

Require validated terminal completion and valid call arguments before accepting a
provider attempt. Keep incomplete output for diagnostics, not execution. Add truncated
SSE fixtures at each block/message boundary, including mutation-shaped calls.

### F2. Admission is neither atomic nor idempotent

`session/turn.rs:108-134` reserves the active session, writes the user message and
each part separately, and publishes events along the way. There is no client
submission ID. A write failure returns before removing the active reservation.

A trigger in a disposable fixture database rejected the first part insert. The
submit returned 500; after removing the trigger, retry returned 409. One user
message with zero parts remained. This directly demonstrates incomplete admission
and a stranded active reservation.

Add a client submission ID, payload hash, one admission transaction and a cleanup
guard. A lost HTTP acknowledgement must permit exact receipt retry. Separately,
`start_call`, `settle` and `finish` discard store errors at `turn.rs:259-263,340-358`;
do not dispatch a mutation or publish a durable-looking success after persistence
fails. There is no durable attempt/input/outbox ledger in the current migrations.

### F3. Failed attempts are replayed as ordinary history

`session/convert.rs:6-16` converts every assistant message regardless of its status.
The runner persists partial blocks on errors/abort. The next request therefore
includes failed-attempt text, signed blocks and tool calls; unfinished calls get a
synthetic interrupted result. Restart marking messages aborted does not exclude them.

Keep audit history separate from replay eligibility. Only validated completed
provider attempts should supply native blocks. Reconcile genuinely committed tool
effects separately rather than treating every incomplete call as the same condition.
Test disconnect/retry and abort/resume through the actual outbound request body.

### F4. Auth rotation and account identity are not durable contracts yet

`session/turn.rs:147-179` resolves/refreshes credentials independently for each
submit, without single-flight ownership or a generation check. Concurrent expired
turns can race refresh; a late refresh can overwrite a newer login. Credential
storage is keyed only by provider in `llm/credentials.rs`, and `ModelRef` has only
provider/model, so key and subscription login replace each other without a pinned
session account/route identity.

Introduce account/access-profile identity before adding more auth modes. Coordinate
refresh, fence logout/replacement and re-read under ownership. Preserve the working
PKCE and known-block paths rather than restarting the adapter implementation.

## Spec

Independent comparison with current `docs/engine-rewrite.md` and `CHECKLIST.md`:

1. **Archive purge falsely reports success.** `src/engine/actions.ts:169-177`
   implements purge by archiving, suppressing errors and returning true. The
   existing retention coordinator then removes its tombstone. This breaks the
   seven-day confirmed-purge rule even if later lifecycle features remain deferred.
2. **Archive authority is split.** Sidebar archive/restore still changes shell
   `session_meta`; native operations change `session.archived_at`. Reconcile those
   paths before presenting the existing archive UI as migrated.
3. **Failed or overlapping hydration can lose events.** The event client advances
   its cursor in `finally` even after hydration failure; concurrent hydrations share
   and replace `held`. The app suppresses snapshot HTTP errors. This does not meet
   the canonical plan's buffered-event recovery contract at lines 94-102.
4. **Resync is incomplete.** It loads only selected-workspace sessions and permissions,
   not authoritative run status. Session lists stop at 200 and are treated as
   complete. Native cutover needs pagination and status/transcript reconciliation,
   especially after missing an idle or terminal event.

The removed legacy purge, permission and reconnect tests need replacement behavioral
coverage. Other M2/M3 features are legitimately deferred; this review does not
require finishing them all to complete the Anthropic vertical slice.

## Standards

One documentation finding: the canonical plan says the existing store/reducer
consume new event types directly, while `src/engine/native/adapt.ts` deliberately
converts to legacy shapes. Document that temporary boundary and its removal gate.
The adapter itself is a reasonable transition, not a reason to rewrite the UI.

**Follow-up:** `d04a59054` now documents this adapter and its M4 removal gate. That
documentation finding is resolved; it is not a current requested change.

The independent standards review found no serious architecture-boundary breach.
New core work remains in `crates/`, the shell shares its connection, and API types
are generated. Standards findings should not obscure the functional failures above.

## Verification

- `cargo test --workspace --offline --locked`: 217 passed, comprising 129 shell
  and 88 engine tests. Headless/doc-test targets had zero tests.
- `cargo clippy --workspace --all-targets --offline --locked -- -D warnings`: passed.
- `bun run typecheck`: passed.
- Selected native actions, adaptation, events and generated-schema suites: 17 passed.
- Two extra black-box fault probes reproduced F1 and F2 through HTTP against
  `drift-engined`, an isolated fake provider and disposable workspace/database.
  The fixture directory and child process were cleaned up. No production account,
  keychain or model inference was used. Probe code remains in the approved temp
  directory as `drift-progress-probe.ts`, not part of the engine.

Standards: 1 documentation finding, resolved during the check, leaving 0 open.
Spec: 4 findings, with false purge success and
failed resync the highest-impact UI issues. Separately, 4 foundational findings
need attention before broadening the implementation; terminal stream validation
and atomic admission have direct failing reproductions.
