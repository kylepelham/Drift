# Rewrite checkpoint at 896deaf

Checked 2026-09-29 at `896deaf4cf048d22fac5ef00e4ea2388f2f7ec34`, following
[the previous fix review](fix-review-b99a6ec.md). No runtime source, canonical plan
or checklist was edited. The current commit fixes several important earlier failures.

## Verified closed or substantially improved

- Durable submission identity and payload hash are stored atomically. The local
  restart probe now returns the original receipt and retains one user message;
  changed text with the same ID returns 409. Sequential retry is fixed.
- The original failed-part admission probe still rolls back, leaves zero messages
  and allows the next submission. No-stop-reason streams still execute zero tools.
- Equal-timestamp pagination now returns both fixture pages instead of losing half
  the rows. A running session on an earlier page remains busy in the actual actions
  module rather than being misclassified idle.
- `hydrateFrom` propagates required loading failures instead of suppressing them.
- Consecutive read groups preserve mutation barriers; the new regression checks that
  a read after a write sees the new contents. Group concurrency is not bounded yet.
- Purge now invokes DELETE, and persisted session/message/todo/submission rows
  cascade. It no longer acknowledges an archive PATCH as a permanent deletion.
- Credential refresh compares the expected stored value before replacing it. This
  catches a login/logout completed before the comparison, but is not a full fence.

## Still open

1. **Resync does not repair cached transcripts.** `hydrateFrom` in
   `src/engine/index.tsx:44-47` loads provider/list/status/ask state, not messages.
   `openSession` in `actions.ts:79-80` skips already-loaded transcripts. A missed
   delta or completion can therefore remain truncated/streaming after successful
   resync. Reload or invalidate loaded message projections at the recovery boundary.

2. **Credential comparison and mutation are not mutually exclusive.** Only
   `replace_if` takes `write_lock` in `llm/credentials.rs:59-80`; `set` and `remove`
   bypass it. A login/logout between comparison and replacement can still lose.
   Every mutation must share ownership, ideally with a captured generation rather
   than only value equality. The new test covers sequential replacement, not the race.

3. **Terminal/replay edges remain.** The local fake-provider probe still executes
   a read when a tool-use stop reason arrives but Anthropic `message_stop` is absent.
   `turn.rs:282-327` also executes calls before checking MaxTokens and does not
   require validated complete tool arguments. All aborted assistant rows still
   replay, including incomplete rows converted to aborted at startup. These are
   distinct from the no-stop-reason case that is now fixed.

Other concurrency boundaries to include before release:

- Submission lookup precedes awaited planning and is not rechecked atomically with
  admission. Concurrent exact retries can return Busy or a uniqueness error instead
  of their receipt; sequential/restart correctness does not cover this interleaving.
- DELETE cancels then deletes without waiting for runner quiescence or excluding
  concurrent admission. An already-dispatched effect or late event can outlive its
  deletion acknowledgement. Require archived state and a coordinated purge boundary.
- The WS held buffer is not fenced by engine instance or disposal. Old-instance
  buffered events can survive a restart during hydration. Local purge cleanup also
  lacks the full deletion revision/ask/activity cleanup used by the event reducer.
- UI send still generates a fresh submission ID per invocation. An unconfirmed
  retry must retain its original ID to use the new durable backend semantics.
- Existing ignored result-persistence errors, unbounded read groups and deferred
  account/archive ownership remain follow-up work, not closed by these fixes.

## Verification

- `cargo test --workspace --offline --locked`: 256 passed, comprising 129 shell
  and 127 engine tests; headless/doc-test targets had zero tests.
- `cargo clippy --workspace --all-targets --offline --locked -- -D warnings`: passed.
- `bun run typecheck`: passed.
- Selected native actions/adaptation/events/generated-schema tests: 23 passed.
- Rebuilt the headless engine and reran the isolated local probes. Changed payload:
  409; restart: same receipt/one prompt; tied pagination: 2 + 2 rows; first-page
  running status: busy. The missing-final-marker probe remains failing as above.
- All probes used fake credentials and a local fake provider; disposable databases,
  files and child processes were cleaned up. No real model, keychain or paid calls.

Assessment: the core fixes are moving in the right direction. Before treating
recovery as complete, finish transcript resync, credential-write fencing and the
terminal/replay eligibility contract, with interleaving/fault fixtures rather than
only sequential happy-path tests.
