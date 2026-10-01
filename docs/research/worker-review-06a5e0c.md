# Worker review at 06a5e0c

Checked on 2026-09-30 at `06a5e0cced77dd94abb367f27b3a3d95bfc521c9`.
The supplied review covered through `cb9c4eef5`. The later task-UI commit does
not fix the engine findings. Runtime source was not modified by this review.

## Supplied findings

All five have support in current source:

1. Individual Stop can miss a worker between `start_task` and the child session
   claim. `stop_task` has no worker-owned token to cancel before that claim.
   This interleaving is source-confirmed, not independently forced by the probe.
2. Completion delivery checks Stop generation before submission, then can wait
   through compaction and admit after Stop. Independently reproduced.
3. `task_output` marks delivery before its tool result is stored, while an
   automatic delivery that already read the row can still attach another result.
   Independently reproduced both acknowledgment-before-attachment and the stale
   automatic delivery.
4. Queued workers reload configuration at execution time. Independently
   reproduced with all four slots occupied: a queued job used a later prompt
   override rather than its launch configuration.
5. Patch rollback excludes the failing step. `undo(index)` restores only
   `steps[..index]`; a truncated or partially written failing destination is
   omitted. Source-confirmed; no disk-full or partial-write fault was injected.

The proposed fixes need coordination through admission, not just extra checks
before waits. Stop can happen during planning or between a check and commit.
Generation validation and admission must share an ordering rule.

## Additional findings

### Foreground results acknowledge before persistence too

`tool/task.rs:95-111` marks the task delivered before returning its output to
the engine for settlement. The same durability fix must cover foreground
results, `task_output` and automatic completion messages. A delivered flag must
mean a durable parent attachment exists, not that a helper constructed output.

### Recovery discards original Stop provenance

`recover_tasks` calls `deliver` with the owner's current generation rather than
the task's launch generation. The latter is not persisted. A valid finished,
undelivered row followed by parent Stop was delivered and started a provider
request when recovery ran in the probe.

This demonstrates the recovery path ignores the earlier Stop; the probe did
not physically restart the process. A restart also resets the owner-generation
map, so it cannot recover the missing cancellation provenance. Persist the
delivery eligibility or cancellation epoch needed to enforce the contract.

### Recovery treats foreground rows as automatic completions

`undelivered_tasks` includes both modes. Foreground start failures call
`end_task` and return an error without marking delivered. The probe constructed
that terminal foreground state and recovery attached a synthetic result to the
parent and woke it.

Automatic background notification and foreground tool-result recovery need
distinct handling, with acknowledgment tied to their respective durable
attachments.

### Replayed launch creates an orphan child

`Task::run` creates a child session before checking whether `create_task` returns
an existing task for the call. Repeating the same launching call produced one
task id but two child sessions. The second child has no corresponding task.

Create or reuse the worker identity and child transactionally. Do not allocate
a new child on a replay. Preserve the correct mode-specific result on reuse.

### Patch reads hide filesystem errors

`apply_patch::existing` treats every read error as absence. Only `NotFound`
should mean the destination has no before-state. Permission, sharing and I/O
errors must fail preparation rather than creating a plan without the bytes
needed for rollback. This is source-confirmed and was not fault-injected.

## Probe results

The temporary external helper links the engine library, uses scripted providers
and disposable file-backed credentials, and invokes public engine/tool APIs.
It does not alter runtime source or contact a paid provider.

```json
{
  "stopDuringDelivery": {
    "newProviderRequestAfterStop": true,
    "resultAttachedAfterStop": true
  },
  "outputDelivery": {
    "automaticAttachmentStillArrived": true,
    "markedBeforeAttachment": true,
    "toolReturnedResult": true
  },
  "recovery": {
    "delivered": true,
    "stoppedParentWoken": true
  },
  "foregroundRecovery": {
    "taskMode": "foreground",
    "unexpectedAutomaticResult": true
  },
  "queuedConfiguration": {
    "usedLatePrompt": true,
    "usedLaunchPrompt": false
  },
  "replayedLaunch": {
    "childSessions": 2,
    "sameTask": true,
    "taskRecords": 1
  }
}
```

Helper location:
`C:\Users\KYLEPE~1\AppData\Local\Temp\opencode\drift-error-body-probe\src\workers.rs`.

## Standards and checks

Multiline comments violate the repository's one-line comment constraint.
Written/canonical shell commands in parallel vectors are a maintainability
concern, not a newly reproduced permission failure; one paired command record
would make the invariant explicit.

- Engine tests: 302 passed, one ignored benchmark.
- Engine Clippy, all targets, warnings denied: passed.
- TypeScript typecheck: passed.
- HTTP/WS conformance: eight passed, 49 assertions.
- `git diff --check`: passed before recording this report.

These tests do not cover the forced delivery interleavings above. The existing
fixes remain useful, but worker lifecycle and patch rollback need the missing
regressions before sign-off.
