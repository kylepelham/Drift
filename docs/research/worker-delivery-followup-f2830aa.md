# Worker delivery follow-up at f2830aa

Checked on 2026-09-30 at `f2830aaae`. Runtime source was not changed.

## 1. Failed automatic delivery has no live retry

`deliver_claimed` logs Busy, UnknownModel and other admission failures. The
automatic claim is released, but no scheduler revisits the owed result.
`release_claims_of` only retries claims that calls held, not a failed automatic
attempt that has already released its claim.

A scripted compaction took 32 seconds. Delivery exhausted its 30-second queue
wait, and after compaction ended the parent was idle with no result attached:

```json
{
  "delivered": false,
  "parentIdle": true,
  "parentResultCount": 0,
  "unconsumedFollowupResponses": 1
}
```

Explicitly invoking recovery then attached the result once and marked it
delivered. No worker/provider effects were replayed by the probe.

Retry pending delivery when the parent becomes available and when a blocking
model/configuration problem is repaired. Coordinate claim release and readiness
notifications so an idle transition racing release cannot lose the retry.
Avoid immediate retry loops on permanent errors.

## 2. Stop suppression uses the delivered flag without attachment

A completed background result followed by parent Stop is marked delivered
without entering the parent transcript. The result remains in the task row,
so the claim that its text is deleted forever is inaccurate. However,
`task_output` reports it as already handed over, and a subsequent user prompt
does not include it.

```json
{
  "storedResultSurvived": true,
  "markedDelivered": true,
  "parentResultCount": 0,
  "taskOutputReturnsResult": false
}
```

Separate wake suppression from successful attachment. Retain completed results
for passive attachment to a later user-initiated turn while preserving the
durable Stop fence against automatic continuation. Update the tests that
currently assert suppression means delivered even with no attachment.

## 3. Foreground consumption needs mode enforcement

`task_output` currently accepts foreground rows and can claim their results.
While that claim is outstanding, the foreground call can return the same result
without `metadata.delivers`. The probe confirmed both behaviors.

Permanent non-delivery is conditional: if `task_output` successfully persists
its result, the transactional acknowledgment marks the task delivered. If that
claim is lost or its write fails after the original call saved an unmarked
result, no live foreground redelivery path closes the row until recovery.

Refuse `task_output` for foreground tasks, as its advertised contract says
background. Preserve the launching foreground call as the result owner.

## 4. Replacement semantics require documentation and metadata handling

Staging then renaming replaces the file object. Hard-link aliases continue to
refer to the old object, Windows handles without delete sharing can prevent
replacement, and a process crash can leave a staging file behind.

Document those limitations and clean up only staging files known to be engine
owned. Windows `Permissions` preserves the read-only attribute, not an arbitrary
original security descriptor. Preserve the original ACL or explicitly fail
when it cannot be preserved; documentation alone does not prevent permissions
from changing. These metadata concerns were source-inspected, not ACL-fault
tested.

## Standards and checks

Multiline comments still violate the repository rule; documentation comments
have no stated exception. The suggested one-line cleanup is valid.

- Engine tests: 313 passed, one ignored benchmark.
- Engine Clippy with all targets and warnings denied: passed.
- Typecheck: passed.
- HTTP/WS conformance: eight passed, 49 assertions.

The local probe uses public engine/tool APIs with scripted providers and
disposable data. Its source is
`C:\Users\KYLEPE~1\AppData\Local\Temp\opencode\drift-error-body-probe\src\pending.rs`.
No production credentials or paid requests were used.
