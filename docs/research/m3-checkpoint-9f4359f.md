# M3 checkpoint at 9f4359f

Reviewed on 2026-09-30 at `9f4359faa660e43f289ad9368106ed75e909fd5e`.
This check covers implemented M3 features, including foreground subagents,
user-reviewed branches, history forks, workspace moves, action models, titles
and compaction. General and explore subagents landed during the review.

Three bugs reproduced through the actual headless engine with a local fake
Anthropic provider and disposable databases. The existing suites pass, but they
do not cover these failure cases. Fixes belong in the implemented features;
pending background workers and other unchecked M3 work are not findings here.

## 1. Compaction drops request context after a failed summary write

Priority: high.

`session/compaction.rs:173-188` ignores failure to insert the summary text, then
marks the summary `done`. It also ignores failure to save the final message.
`view` at lines 47-59 chooses the latest `done` summary without checking that
its text exists. This can discard the previous request history with nothing
useful to replace it.

### Reproduction

1. Complete a turn containing `ORIGINAL_REQUIREMENT_MUST_SURVIVE`.
2. Install a disposable SQLite trigger that rejects text-part inserts whose
   owning message has `summary = 1`.
3. Request manual compaction and let the fake provider return a valid summary.
4. Wait for idle, remove the trigger and submit another turn.
5. Inspect both the database and the provider's next request.

Observed:

```json
{
  "accepted": 202,
  "summaryStatus": "done",
  "summaryParts": 0,
  "originalRetainedInDatabase": true,
  "originalReplayed": false,
  "hasSummaryText": false
}
```

The original rows survive. The bug is loss of their content from subsequent
model requests, not deletion from storage.

### Required fix and regression

Make saving the summary text and publishing its completed state one successful
storage operation. Return persistence errors to the compaction caller instead
of reporting success. Failed publication must leave the prior request view in
use and must not emit a successful completion event.

Add fault coverage for both the text insert and final-message save. Assert that
the old request context remains available, the failed summary does not become
active and automatic failure accounting sees the persistence failure.

## 2. Move accepts a session whose turn is still preparing

Priority: high.

`session/turn.rs:160-162` awaits planning before claiming the session. Planning
captures the workspace and config at lines 219-225, then awaits provider and
credential resolution. Meanwhile, `session/tree.rs:59-63` sees no running turn
and accepts a move. The prepared turn subsequently claims the session and runs
with the old workspace path.

### Reproduction

1. Create source and destination workspaces and a session in the source.
2. Give the isolated file credential backend an expired synthetic OAuth token.
3. Submit a turn and hold the fake token-refresh response open.
4. Move the session to the destination while refresh is pending.
5. Release refresh and let the fake provider request a relative file write.
6. Inspect the stored session location and the written file.

Observed:

```json
{
  "moveStatus": 200,
  "turnStatus": 202,
  "sessionInDestination": true,
  "wroteOldWorkspace": true,
  "wroteNewWorkspace": false
}
```

This is an accepted turn acting in a different workspace from the session's
persisted location. Its captured config also comes from the old workspace.

### Required fix and regression

Coordinate turn preparation and workspace movement under the same ownership
rule. Reserve the session before asynchronous planning or revalidate its
location under an admission gate before accepting and dispatching the turn.
The move's busy check and update must also exclude concurrent admission.

Keep the delayed-refresh reproduction as a deterministic regression. A move
must either be refused during preparation or cause preparation to restart using
the new workspace. No accepted turn may retain the old path and config after
an accepted move.

## 3. A failed subagent returns its compaction summary as its answer

Priority: high.

`tool/task.rs:129-136` chooses the last assistant message marked `done` without
excluding summaries or checking the child's final outcome. It only reports an
error when no completed assistant message exists. A successful compaction
therefore hides a later child failure.

### Reproduction

1. Have the parent call `task` with the general subagent.
2. Make the fake provider reject the child's first request as too long.
3. Let overflow recovery produce `SUBAGENT_FAIL_MARK_SUMMARY`.
4. Reject the child's next normal request as too long again.
5. Wait for the child and inspect the task result supplied to the parent.

Observed:

```json
{
  "childFinalStatus": "error",
  "childFinalError": "invalid_request_error (400): prompt is too long: fixture overflow",
  "taskStatus": "done",
  "returnedSummaryInsteadOfFailure": true
}
```

The parent receives the internal summary without the failure. The child did
not produce a successful final answer.

### Required fix and regression

Derive the worker result from the child turn's terminal outcome. A compaction
summary is not a worker answer. Do not fall back to an earlier successful
message when the worker's final attempt failed or was aborted.

Add the overflow, successful summary, repeated overflow sequence to foreground
task tests. Assert that the result reports the final failure and never returns
the summary as the worker's answer. Cover cancellation after compaction too.
The shared reply helper also needs to exclude summaries when reporting a
branched conversation's latest reply.

## Verification

Checks against the reviewed tree:

- `cargo test --workspace --offline --locked`: 309 passed, including 180 engine
  and 129 shell tests.
- `cargo clippy --workspace --all-targets --offline --locked -- -D warnings`:
  passed.
- `bun run typecheck`: passed.
- `bun test tests`: 1,197 passed, 8,461 assertions across 56 files.
- `git diff --check`: passed before recording this report.

The combined probe is a temporary local helper at
`C:\Users\KYLEPE~1\AppData\Local\Temp\opencode\drift-m3-fault-check.ts`.
It uses fake Messages and OAuth endpoints, fixture credentials, temporary
workspaces and databases, then stops its child process and server and removes
its disposable data. It made no paid provider requests. All three observed
results above came from the same successful run.

These checks do not establish native desktop UI behavior. The report records
failures and required regression coverage; it does not implement runtime fixes.

## Scope decision

M1 and M2 scoped sign-offs remain intact. M3 has made substantial progress and
the current suites are green, but these three implemented-feature regressions
need fixes. Continue the remaining M3 work alongside those fixes. Background
worker implementation remains pending under its existing contract, rather
than being counted as a regression in this checkpoint.

## Fix verification at 4ca12e25b

Follow-up on 2026-09-30 at `4ca12e25b`, covering the fixes in `9d9b98214`.
Rebuilt `drift-engined` and reran the original external fault probes.

| Original reproduction | Result after the fix |
| --- | --- |
| Failed summary text insert | Summary is `error`, has no parts and the next request still includes the original requirement. |
| Move during credential refresh | Move returns 409. The session stays in its source workspace and the admitted turn writes there. |
| Repeated overflow after successful child compaction | Child is `error`; task is also `error` and does not return the summary. |

The code now stores summary text and completed state in one transaction, returns
publication failures and ignores empty summaries in the request view. Added
tests cover failure of either write and automatic failure accounting.

Turn ownership now starts before planning. The move check and database update
hold the same claim lock, excluding admission between them. A new delayed
planning test verifies refusal and a successful move once idle.

Task results now skip summaries and inspect the latest ordinary assistant
message. This fixes the repeated-overflow reproduction and retains the failed
child's transcript link. It does not fully establish the completed turn's
outcome, as the following cancellation probe demonstrates.

### Remaining bug: child-only Stop during automatic compaction reports success

Priority: high.

1. Have the parent start a foreground general subagent.
2. Let the child emit `PARTIAL_PROGRESS_NOT_FINAL` plus a `read` call. Report
   enough input usage to trigger automatic compaction before its next request.
3. Complete the read, then hold the fake summary response open.
4. Call `POST /sessions/{childId}/abort`, leaving the parent running.
5. Wait for the child and parent to become idle and inspect the task result.

Observed against the fixed engine:

```json
{
  "abortAccepted": true,
  "summaryStatus": "aborted",
  "lastOrdinaryStatus": "done",
  "taskStatus": "done",
  "reportedOutcome": "replied",
  "returnedPartialProgress": true
}
```

`session/turn.rs:340-349` exits the turn after cancellation during automatic
compaction without recording an ordinary aborted assistant message.
`tool/task.rs:149-163` skips the aborted summary and sees the earlier completed
tool-use message. It treats that message's progress text as a final answer.
The parent receives a successful task result even though the child was stopped
before its next normal request.

The new cancellation regression aborts the parent after successful compaction.
That takes the separate `ctx.abort.is_cancelled()` path in `Task::run` and does
not exercise child-only cancellation during compaction.

Task completion needs the completed child turn's outcome, not just its latest
ordinary model message. Record or expose cancellation between requests and
return `outcome: stopped` with a failed task status. Retaining partial text for
inspection is fine; it must not be returned as a successful final answer.

Add the exact child-only Stop sequence above as a regression. Keep the parent
active and assert that the task retains its child link, reports `stopped` and
does not classify the progress text as the completed result. This belongs to
existing foreground workers and does not require background-worker machinery.

### Follow-up checks

- `cargo test --workspace --offline --locked`: 314 passed, including 185 engine
  and 129 shell tests.
- `cargo clippy --workspace --all-targets --offline --locked -- -D warnings`:
  passed.
- `bun run typecheck`: passed.
- `bun test tests`: 1,197 passed, 8,461 assertions across 56 files.
- The rebuilt headless engine passed all three original external probes. The
  extended helper reproduced the child-only Stop failure in the same run.

The original reproductions are fixed. The foreground worker result finding
remains open for cancellation during automatic compaction.

## Cancellation fix verification at bcf770de4

Follow-up on 2026-09-30 at `bcf770de4`. The turn loop now records a hidden
worker's ending, with cancellation taking precedence over earlier completed
model messages. The foreground task consumes that outcome when producing its
result. Failed and aborted summaries also remain visible to the reply helper
as failures or stops; only completed summaries are skipped.

Rebuilt the headless engine and reran the exact child-only Stop probe:

```json
{
  "abortAccepted": true,
  "summaryStatus": "aborted",
  "lastOrdinaryStatus": "done",
  "taskStatus": "error",
  "reportedOutcome": "stopped",
  "returnedPartialProgress": false
}
```

The parent stays active, receives a stopped worker result and does not mistake
the earlier progress text for a final answer. The added child-only cancellation
tests pass for both automatic compaction and a running shell tool, retaining
the stopped child's transcript link.

All three original external fault probes still pass. Checks:

- `cargo test --workspace --offline --locked`: 316 passed, including 187 engine
  and 129 shell tests.
- `cargo clippy --workspace --all-targets --offline --locked -- -D warnings`:
  passed.
- `bun run typecheck`: passed.
- `bun test tests`: 1,197 passed, 8,461 assertions across 56 files.

All reproduced findings in this checkpoint are now closed. No further
implemented-feature failure was reproduced in this follow-up. Continue the
unchecked M3 work under the existing plan; this closes the scoped findings,
not the milestone as a whole.
