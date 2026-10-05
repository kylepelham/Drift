# Agent-loop review at b93cbc9

Reviewed on 2026-09-30. Work landed during inspection, from `1739dd08e` through
`b93cbc9e5abf8c3e30b66e6c85e281a134a1110f`. The final headless probe was rebuilt
and rerun against `b93cbc9`. This review does not modify runtime code.

## Reproduced findings from the supplied review

| Finding | Local reproduction | Required behavior |
| --- | --- | --- |
| Undo loses intermediate user edits | Agent writes a file, user edits it, agent reads and writes it again. Undo to the first prompt returns 200 with `kept: []` and restores the original contents. | Validate continuity between every change's `after` and the next `before`; preserve and report a broken chain in both undo and redo. |
| Patch bypasses read-before-write | `Add File` overwrites an unread existing fixture and reports `done`. | Require the same existing-file read checks as edit/write, including update, deletion and existing move destinations. |
| Patch exposes an unread sensitive file | Deleting a synthetic `.env` returns its contents in the tool result under an edit-allow rule. | Read approval must govern reading or disclosing prior contents. Edit approval must not imply secret-read approval. |
| Own output requires another ask | A shell result spills to the session's output directory. Reading its reported path produces a new read permission request. | Permit reads of owned artifacts with canonical-path and ownership checks, without opening the whole data directory. |
| Assignment-prefixed commands miss deny rules | A deny for `git status*` does not refuse `FIXTURE=1 git status`; it asks instead. The test aborts the request without running it. | Normalise safe leading assignments and supported aliases before policy matching; retain the complete original command for approval. |
| Moving breaks historical undo | Write a fixture in workspace A, put its after-state in B, move the session to B and undo. Move returns 200; undo returns 500 because B has no source shadow repository. | Snapshot references need stable storage ownership independent of the session's current workspace path. Define where historical undo applies after relocation. |

The final-path comparison in `session/revert.rs` does not repair the undo chain
problem: the disk contents can match the final agent write even though an
intermediate user edit was incorporated. `net_changes` currently replaces
`existing.after` without checking continuity.

The move finding is broader than an ineffective undo: some cases report kept
paths, while the reproduced matching-after-state case returns a file error.
Retention also looks up references through the session's current workspace,
although the objects remain in the original path-keyed shadow repository.

## Additional reproduced findings

### Patch move destination is absent from the permission ask

`tool/apply_patch.rs` includes move destinations in `touches`, but not in `ask`.
A patch updating `source.txt` and moving it to a path denied by an edit rule
reports `done` and creates that destination. The reproduction used harmless
fixture text and a denied filename within a disposable workspace.

Evaluate every resolved source and destination under the relevant rules. The
combined newline pattern is not a substitute for per-path decisions.

### Failed multi-file patch leaves earlier files changed

`ApplyPatch::run` applies operations sequentially. A patch adding one file and
then failing a hunk in a second file reports `error`, but the first file remains
created. Validate and prepare the entire patch before mutation, with explicit
failure handling for writes that fail after validation.

### SSE decoding corrupts UTF-8 split across network chunks

`llm/sse.rs:23-24` performs lossy decoding on each incoming chunk. A split inside
the euro sign changed persisted model text from `LEFT € RIGHT` to
`LEFT ��� RIGHT`. A JSON string can remain valid while its content changes;
tool arguments and file paths are also susceptible.

Keep incomplete UTF-8 bytes between chunks and decode complete sequences only.
Add coverage at every split position of multibyte text and tool arguments.

### Truncated worker output is classified as a successful answer

A foreground worker returns text with `max_tokens` as its stop reason. Its
message is `done` with an output-limit error, but the parent task is `done`,
reports `outcome: replied` and returns the incomplete text as its answer.

`record_end` and `last_attempt` use status without respecting this terminal
reason. Preserve incomplete output for inspection, but do not present it as a
successful worker completion. This affects the existing foreground path too.

### Mentioned file contents do not satisfy read-before-edit

Admission includes the contents of an ordinary workspace file named by an
`@` file reference. The model immediately edits that file and receives
`has not been read this session; read it before editing`.

`session/attach.rs` reads outside the `SessionFiles` ledger. Record approved
file reads consistently, with explicit semantics for truncated mentions.

### Malformed attachments are admitted and disappear

A text attachment with invalid base64 receives an accepted admission. Conversion
then drops it because `data_text` returns `None`. The provider gets only the
accompanying text, and the turn completes without an attachment error.

Validate MIME/data-URL consistency and payload decoding before admission.
Accepted input must not disappear silently during conversion.

### Non-success response bodies bypass the new timeout protection

Provider adapters call `response.text().await` after receiving error headers.
That operation is outside both the header timeout and the SSE idle watcher.

A standalone adapter probe configured 50 ms header and idle limits. The fake
endpoint sent 429 headers plus an incomplete body, then held the connection.
The adapter still waited past an outer 300 ms deadline. The probe aborted its
local server afterward. The same error-body pattern exists in all adapters.

Bound error-body waiting and size. Stop working here does not replace automatic
stall detection.

## Source-confirmed concerns and qualifications

- Runner and installer subcommands are still eligible for widening in
  `tool/command.rs`. Treat code-executing commands and privilege-changing
  arguments conservatively; do not assume a shared subcommand grants equivalent
  authority.
- `recorded_blobs` holds the shared database lock while scanning and parsing
  matching JSON. The mechanism is confirmed; its latency is not measured.
  Collect rows with a short lock at minimum, or use explicit indexed references.
- Local adapters use the same 120 s header and 300 s idle limits. Cold-load and
  long-prefill failures are plausible but were not reproduced against a real
  local model. Support route-specific configurable limits.
- `Deny and stop` cancels the worker's token, not its parent's. Whether that
  should stop the owner is a product decision. Label and document the scope.
- Steering validates attachments against the newly selected model but the
  in-flight plan continues on its old model. This needs active-model validation
  or a deliberate boundary switch, not merely a UI notice.
- OpenAI requests do not include a stable `prompt_cache_key`. That is a routing
  improvement candidate, not proof that OpenAI's automatic caching never hits.
- The normal native UI already advertises audio/video input as false in
  `adaptModel`. Its capability gate blocks them. Direct engine admission refuses
  them too; the refusal wording should still give a useful remedy.
- `large_files` does not use `require_git(false)`, unlike the ordinary tool walk.
  Ignored directories in non-Git workspaces can be traversed unnecessarily.
- The one-at-a-time statement in the rewrite document is stale. Reads and tasks
  run concurrently. Shadow-index locking does not coordinate edits to the same
  source file by different workers.
- Mention reading loads a complete file before truncating, and uses synchronous
  filesystem reads during admission. Bound source reads as well as the emitted
  context; the current output cap does not bound this allocation.

## Verification

- `cargo test --workspace --offline --locked`: 409 passed, one ignored benchmark
  test, including 280 engine tests and 129 shell tests.
- Workspace Clippy with all targets and warnings denied: passed at final HEAD.
- Typecheck: passed at final HEAD.
- Full Bun suite during the review: 1,204 passed. The final conformance rerun
  after worker work landed: eight passed, 49 assertions across two files.
- The initial checks caught transient tool-count and generated-schema failures
  while another agent was implementing workers. Both were resolved before the
  final checks.

The external headless reproduction helper is
`C:\Users\KYLEPE~1\AppData\Local\Temp\opencode\drift-review-round3.ts`.
The standalone timeout probe is under `drift-error-body-probe` beside it. Both
use only local endpoints, fixture credentials and disposable data. No production
secrets or paid provider requests were used. No native UI smoke test is claimed.
