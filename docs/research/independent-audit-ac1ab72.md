# Independent audit at ac1ab72

Independent source and headless audit on 2026-10-01, checked at
`ac1ab720d172c16c1a0198e585a28df1a57cd9e2`. No runtime source edits or browser
sessions were used. Earlier UI findings were not recycled into this report.

## 1. Undo/redo marker failure leaves files and history inconsistent

Priority: high.

`session/revert.rs` completes `shift` before calling `mark`. The compensation
path covers errors during file restoration, not a failed database marker save.

After a successful agent write, a temporary trigger rejected the session's
revert marker update. Undo returned a database error but restored the original
file; the session still had no revert marker. After removing the trigger, a
retry called the file externally edited and reported it as kept.

```json
{
  "undoMarkerFailure": {
    "file": "ORIGINAL",
    "returnedError": true,
    "revertMarkerSaved": false
  },
  "undoRetry": { "kept": ["a.txt"] }
}
```

Carry the applied-file journal through marker persistence and compensate on a
marker-save failure too. Keep recovery information when compensation fails.
Test the same boundary in redo. Do not report a successful file/history
transition until both sides agree.

## 2. An old queue failure poisons a newer replacement

Priority: high.

`start_queued` reads rows, then awaits admission/planning. On failure,
`fail_queue` updates every current row for that session, not just the attempted
submissions. Replacement is checked on successful admission, but not on the
failure path.

The probe started an old queued Plan prompt with an expired synthetic Anthropic
credential and held its local refresh response. A valid Build prompt on OpenAI
replaced it while planning waited. When the old refresh failed, the new prompt
was marked `provider has no credentials`, despite its own usable key. No model
request ran for the replacement.

```json
{
  "returnedOldParts": true,
  "queued": {
    "agent": "build",
    "error": "provider has no credentials",
    "model": { "provider": "openai", "model": "gpt-4.1" },
    "text": "NEW_VALID_GOAL"
  },
  "providerRequests": 0
}
```

Fence failure updates by the exact attempted queue identity/version. If those
rows were replaced, leave the new queue untouched and schedule its admission.
Use a conditional transaction, not a check separated from the update.

## 3. Post-write capture errors silently remove undo information

Priority: high.

`session/turn.rs` converts `capture_after` errors to `None` with `.ok()`. The
tool retains a successful status without changes or an unrecorded warning.
`capture_before` limits the previous file, but not the prepared new contents.

Two probes reproduced this:

- A write grew an existing small file past 10 MB, reported `Done` and left no
  recorded changes or unrecorded warning. Undo succeeded but left the large
  file unchanged.
- An edit used a 256-character replacement with `replace_all` on a 50 KB file.
  This produced 12.8 MB with ordinary-sized tool arguments. It likewise
  reported `Done`, had no change record and survived undo.

```json
{
  "editExpansion": {
    "hasRecordedChanges": false,
    "sizeAfterUndo": 12800000,
    "status": "Done"
  }
}
```

Validate prepared output sizes before mutation. Never discard after-capture
failures silently: retain before-state and report incomplete history, then
compensate or record enough state to recover safely. Cover unrelated capture
I/O failures as well as size rejection.

## 4. Worker mutations are ordered by message creation, not execution

Priority: medium.

`net_changes` sorts by assistant-message and part ids, which are allocated
before tools run. Two workers can create their messages in one order and make
their sequential file changes in the reverse order.

The probe delayed worker A's write response. Worker B wrote first; A captured
B's resulting contents and wrote last. Both changes belonged to the parent,
with no intervening user edit or overlapping file write. Undo nevertheless
reported the file as kept and left A's final contents, because sorting by
message creation manufactured a broken chain.

```json
{
  "beforeUndo": "LAST_AGENT_A",
  "afterUndo": "LAST_AGENT_A",
  "kept": ["shared.txt"]
}
```

Record mutation ordering at the actual capture/apply boundary. Preserve genuine
broken-chain detection; do not treat stream start ids as effect order.

## 5. Returned drafts lose file mentions

Priority: medium.

The actual `returnedPrompt` and `restoreComposerDraft` functions were called
with a queued prompt holding text plus a `file:` reference. Restoration retained
the visible `@src/a.txt` text but returned no mentions and no staged files.
Subsequent composer submission therefore does not expand that reference.

```json
{
  "text": "Check @src/a.txt before proceeding",
  "mentions": [],
  "stagedFiles": 0
}
```

Preserve reference parts/provenance or reconstruct the mention list while
restoring returned prompts. Test both Stop and Discard with references and
ordinary attachments.

## Verification and scope

- Engine tests: 417 passed, one ignored benchmark.
- Engine Clippy with all targets and warnings denied: passed.
- TypeScript typecheck: passed.
- Focused native-action tests: 26 passed, 104 assertions.
- The independent runtime probe reproduced the first four findings. A direct
  invocation of frontend functions reproduced the fifth.
- Queued-model projection was separately checked and now selects the queued
  model correctly; it is not an open finding here.

Builds used a temporary target directory rather than the shared repository
target. Fixtures used scripted providers, a local synthetic token endpoint and
disposable file-backed credentials. No production credentials or paid model
requests were used. Runtime tests covered the latest source as it landed during
the audit; all reported paths still matched the final clean working tree.

Probe sources are under
`C:\Users\KYLEPE~1\AppData\Local\Temp\opencode`:

- `drift-error-body-probe/src/independent.rs`
- `drift-independent-ui-check.ts`

## Fix verification at c02d9c49d

Follow-up on 2026-10-01. Inspected the five fix commits and reran the original
independent probes against `c02d9c49d`.

| Original finding | Result after the fix |
| --- | --- |
| Failed undo marker save | Undo still reports the database error, but the file stays `AGENT_CHANGED` and no marker is saved. After removing the fault, retry has `kept: []`. Added coverage includes redo too. |
| Oversized write and replace-all expansion | Both calls report `Error` before changing the files. Sizes after the undo probe are the original eight bytes and 50,000 bytes. Capture failures also have compensation and explicit history-error coverage. |
| Old queue failure poisoning its replacement | The old parts are returned; the new goal reaches one model request and the queue empties without the old credential error. |
| Incorrect worker mutation ordering | Undo restores `ORIGINAL` with `kept: []`, despite the reverse order of message creation and writes. |
| Returned file mention losing its reference | The restored draft includes `mentions: ["src/a.txt"]`. The probe supplies the workspace root required by the updated function; application callers now supply it too. |

All five reproduced findings are closed in this scoped follow-up. No new
failure was reproduced by these checks.

`bun run gates` passed all checks: headless build, generated-client consistency,
workspace Clippy, TypeScript typecheck, Bun tests and workspace Rust tests.
The first gate invocation had a PowerShell parsing error before running any
gate; the corrected invocation passed in 21.2 seconds. Cloud credential lookup
was redirected to absent fixture paths and AWS environment credentials cleared
for the gate process. No browser was opened.
