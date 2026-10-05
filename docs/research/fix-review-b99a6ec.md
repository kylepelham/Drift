# Failure-path fix verification at b99a6ec

Checked 2026-09-29 at `b99a6ec8febb7e07dcdb320b7ef6134ea1758c75`. This follows
[the previous progress review](progress-review-772fe5c.md) and checks `e730c07ef`,
`11a313044` and the note in `docs/engine-rewrite.md:202-223`. No runtime source or
canonical milestone documents were changed by this review.

## Verdict

The original two black-box failures are fixed. Admission rolls back and releases
the session; a stream with no stop reason does not dispatch its calls. The deferred
account/archive note is useful, but narrower correctness gaps remain outside those
deferrals. Do not label the whole failure-path contract settled yet.

## Previous findings: actual progress

| Finding | Verified change | Remaining boundary |
| --- | --- | --- |
| F1: EOF without a reason executes tools | Repeated fake-provider EOFs without a stop reason produced error attempts and zero completed tools. | A reason is not the terminal marker; partial calls and MaxTokens need separate dispatch rules. |
| F2: failed admission leaves partial input and Busy | Injected part-insert failure returned 500, left zero messages, and the next submission returned 202. | Submission receipts are process-local and not checked against changed content. |
| F3: failed attempts enter model history | Error and streaming assistant rows are excluded. | All aborted rows remain eligible, including unfinished messages made aborted during restart. |
| F4: parallel turns race refresh | A per-provider mutex and stored-token re-read serialize turn-versus-turn refresh; regression test passes. | Login/logout does not share that ownership or fence the refresh write. Profiles are explicitly deferred. |
| Hydration advances on callback failure | Socket helper retains the cursor and retries a rejecting callback; overlapping resync is serialized. | Production hydration swallows required HTTP errors, so the helper sees success. |
| Listings truncate at 200 | Client requests successive pages and obtains running flags. | Server timestamp ties are skipped; running IDs come only from the final page. |
| Purge claims success on request failure | Failed archive requests return false. | Successful archive still returns true to a coordinator expecting confirmed deletion. |

## Outstanding fixes

### 1. Durable submission identity and payload validation

`session/turn.rs:83-88,119-155` keeps receipts only in `Turns.receipts`. Admission
in `store/sessions.rs:394-420` persists neither ID nor payload hash. Local probes:

- Same ID/session but different text returned 202 with the original receipt.
- Same original ID/text after restart returned a different receipt and created a
  second user message.

Persist ID, session, payload hash and receipt in the admission transaction. Exact
retries return that receipt; changed payload conflicts. UI retries of an unconfirmed
send must reuse the action's ID instead of creating a new one.

### 2. Real UI hydration still hides failures

`src/engine/index.tsx:58-66` catches session-list and permission-load failures and
continues. `src/engine/native/events.ts:46-49` therefore advances its cursor despite
missing required state. New helper tests reject their callback but do not exercise
this production integration. Propagate required snapshot failures and test the real
callback. Bound held events when an endpoint remains unavailable.

### 3. Paging loses tied rows and earlier-page running state

The server orders by `updated_at` only and uses a strict earlier timestamp for
continuation in `store/sessions.rs:75-80`. Four fixture sessions with the same
timestamp produced two rows on page one and zero on page two. Two were skipped.
Use a deterministic composite cursor such as `(updated_at, id)`.

`src/engine/actions.ts:114-121` accumulates sessions but collects running IDs only
from the last page. A probe through the actual `createActions` loaded 201 sessions
with a running session on page one; its final status was idle. Accumulate running
IDs across all pages. The current regression places its running session on the
final page and misses this case.

### 4. Replay and terminal contracts remain too broad

A fake Anthropic stream with a `tool_use` stop reason but no `message_stop` still
executed its read. This is narrower than the original fixed case. Decide terminal
completion explicitly per adapter rather than equating iterator EOF with success.

`session/turn.rs:275-278` runs calls before checking MaxTokens, and the assembler
tolerates invalid tool JSON. Require complete validated calls and an eligible stop
outcome before dispatch. `session/convert.rs:6-19` admits every aborted assistant
row; startup converts unfinished streaming rows to aborted. Preserve completed
blocks where the provider permits it, but do not treat all interrupted signed or
tool blocks as complete merely because their attempt is now named aborted.

### 5. Single-flight refresh does not fence login/logout

`session/turn.rs:198-212` prevents simultaneous refreshes, but key/OAuth/remove
routes write independently and the refresh result is stored unconditionally. A
new login or logout during the request can still be overwritten. This is separate
from multiple-account support. Add generations or compare-before-store fencing
with a deterministic replacement-during-refresh test. Ignored store failures in
`turn.rs:282-286,389-407` also remain a gap before dispatch and success publication.

### 6. Parallel reads must respect mutation barriers

`session/turn.rs:317-333` partitions all reads ahead of all writes. This turns
`write a; read a` into `read a; write a`. Parallelize independent read groups while
preserving mutation barriers, or use explicit dependencies. The current fixture
uses unrelated files and does not test dependent reads.

## Are the explicit deferrals reasonable?

**One credential slot until M3:** acceptable for constrained development, not a
completed multi-account/access-mode contract. Define profile/replay ownership
before additional account-switching flows depend on the store. Fencing current
logout/refresh races should not wait for a profile picker.

**Archive consolidation at M4:** keeping two tables temporarily is reasonable.
Incorrect purge acknowledgements are not. The current archive PATCH can return
true from `purgeSession`, causing the seven-day coordinator to clear its deletion
tombstone. Disable that completion path or retain the tombstone until deletion
exists. Archive/restore also needs one behavioral owner before being called finished.

## Verification

- `cargo test --workspace --offline --locked`: 252 passed, 129 shell and 123 engine.
  Headless/doc-test targets had zero tests.
- `cargo clippy --workspace --all-targets --offline --locked -- -D warnings`: passed.
- `bun run typecheck`: passed.
- Selected native actions/adaptation/events/generated-schema tests: 22 passed.
- Rebuilt the headless engine and ran fake-provider probes for original EOF and
  admission failures, missing final marker, changed submission payload, retry
  across restart and tied pagination. A synthetic client probe exercised earlier-page
  status through the actual actions module.
- All engine probe data was disposable and cleaned up. No real model, account,
  keychain or paid inference was used. Helpers stay outside git in the approved
  temp directory as `drift-fix-recheck.ts` and `drift-status-recheck.ts`.
