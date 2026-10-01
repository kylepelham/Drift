# M3 provider, question and MCP review at 83dacc0

Checked on 2026-10-01 at `83dacc0e6`. Runtime source was not changed.
The supplied review's main findings are supported by current source. The
external probe independently exercised the cases described below.

## Provider findings

### Compatible streams invent terminal reasons

`compat::StreamState::done` emits a stop reason even when no `finish_reason`
arrived. A local endpoint returned a valid write call and `[DONE]`, without a
finish reason. The actual engine wrote the fixture file. This violates the M1
terminal contract and affects all compatible routes, not only OpenRouter.

Reject completion without the provider's terminal reason. Keep its calls
unexecuted and report a broken stream.

### Additional: interleaved tool deltas become duplicate call fragments

Two calls began in one chunk, then each continued in another. The adapter
emitted four starts instead of two:

```json
[["a", "read"], ["b", "glob"], ["a", ""], ["b", ""]]
```

State is kept for one open block, rather than complete call state per index.
Maintain each call's id, name and accumulated arguments across interleaving and
emit one assembled call per provider call. Add a multi-call wire regression.

### Google token requests ignore bounds and transient classifications

The token exchange uses direct `send` and `json`, outside the route's header
and body limits. With Vertex header and idle limits configured to 50 ms, an
incomplete local token response still waited beyond a 300 ms outer deadline.
A JSON 503 response was classified as unauthenticated and lost its retry wait.

Use the shared bounded HTTP path, preserving status, error kind and response
headers. Stop already cancels the enclosing provider request; that does not
replace automatic timeout protection.

### Additional: the token cache ignores replacement at the same path

The cache checks credential path and expiry only. Replacing a fixture ADC file
at the same path with a different client identity returned the old token; the
token server saw one request, not two. Cache identity must include credential
version/content, and an older mint must not overwrite newer credentials' cache.
Coordinate concurrent minting for the same version.

### Bedrock errors are dropped or not retried

Local valid AWS event-stream frames reproduced:

| Frame | Result |
| --- | --- |
| Exception `internalServerException` | One error, non-retryable. |
| Exception `modelStreamErrorException`, original status 503 | One error, non-retryable. |
| Error `InternalFailure` | No chunks and no error. |

Classify exceptions and error frames, preserving their error-code/message
headers and original status where supplied. Do not make validation or auth
failures retryable merely because they arrived inside a stream.

## Async question findings

### Answer and dismiss can both succeed

An answer waited for compaction to finish. Dismiss returned success and removed
the card while it waited. The original answer then returned success too and
saved a clarification prompt. Coordinate answer and dismiss under one
request-scoped lifecycle claim; closing a card must correspond to the operation
that actually won.

### Concurrent identical answers fail the submission constraint

Forty paired submissions on independent threads each produced one
`UNIQUE constraint failed: submission.id` error. The question's accepted answer
was otherwise identical. Submission replay lookup and payload comparison need
to occur inside the admission transaction. Same payload must replay; a
different payload must produce a conflict.

### Reopening loses replay success for a saved answer

After saving an answer, reopening the engine and resending the same answer
returned `NotPending`. Pending cards may be process-local under the current
contract; completed-answer replay cannot depend on that in-memory registry.
Use the saved `answer:<id>` submission and clarification content to recover
the decision before returning 404.

### WebSocket failures are not acknowledged

The socket spawns `answer_question` and discards its result. This is
source-confirmed, not socket-fault injected. Add request-correlated success and
failure feedback so socket clients can distinguish saved, conflicting and
failed answers. Preserve the save-before-close rule.

## MCP findings and the user's decision

The user specified that disabling should stop active use, while
disconnect/restart should preserve continuity.

### Stale row starts under a newer generation

The probe read an approved server row, disabled/disconnected the stored server,
then continued connecting from the captured row. Connect returned success,
although the current stored row was disabled, and two tools were published.

Capture configuration and generation consistently. Validate and publish under
one state lock, including failure states. The source also confirms the
generation-check/client-insert gap described in the supplied review; that
second interleaving was not separately forced.

### Captured tools remain callable after disable

The probe retained a tool object before disable. After disable/disconnect it
still successfully called its old server. Revocation needs to reach retained
tool objects and calls in progress, rather than only removing the global map.

### Captured tools do not follow a replacement connection

After disconnecting and connecting a replacement, the old captured client was
made to exit. Its tool failed, while the tool from the replacement connection
worked. Running turns need a continuity mechanism for the same approved server
definition. Reconnect must not automatically repeat uncertain mutating calls.

### Reconnect waits and crash-loop backoff

Initialization and tool discovery have no engine-owned timeout, and invalidation
does not cancel a pending open. A hung attempt can retain the engine and prevent
further retry. These are source-confirmed, not long-duration hang injected.

Backoff is local to a reconnect attempt. Each successful handshake creates a
fresh watcher, so short-lived successful connections reset retry history.
Keep per-server backoff until the connection has been stable, bound attempts,
and cancel stale attempts with cleanup and waiter notification.

## Snapshot and release housekeeping

The skill probe froze configuration, changed a known skill's `SKILL.md`, then
called the skill. The changed body was returned. Pin instructions as well as
skill name/path/description under the documented runtime snapshot contract.

Research docs remain untracked while tracked files link to them. Include the
referenced documentation in release history through the normal requested
commit workflow; do not silently commit unrelated research.

Recorded/local provider tests do not verify live Bedrock/Vertex account flows
or the outstanding live Gemini path. Arrange authorised live smoke checks
before M4 removes the fallback. This review made no paid requests.

## Verification

- Engine tests: 366 passed, one ignored benchmark.
- Engine Clippy, all targets, warnings denied: passed.
- Typecheck: passed.
- HTTP/WS conformance: eight passed, 49 assertions.

Probe source is
`C:\Users\KYLEPE~1\AppData\Local\Temp\opencode\drift-error-body-probe\src\m3.rs`.
It uses local HTTP endpoints, scripted model providers, the local MCP echo
fixture, synthetic credentials and disposable data. Google credential lookup
was explicitly redirected to a fixture file; no production cloud secrets were
used. No native UI smoke test is claimed.
