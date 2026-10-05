# Staged recovery review at e92100f

Checked on 2026-09-30 at `e92100fc2`. Runtime source was not modified.

## Confirmed startup data loss

`tool/stage.rs::clean_leftovers` deletes recorded `.bak` and `.tmp` files
without checking the original destination. The live replacement path has an
orphan-backup check, but startup does not.

The probe constructed the relevant recoverable state: destination absent,
backup containing original bytes, temporary file containing replacement bytes,
both paths registered in SQLite. It dropped and reopened the engine. Startup
removed both files and their records, leaving the original destination absent.

```json
{
  "backupExists": false,
  "remainingRecordedFiles": 0,
  "targetExists": false,
  "temporaryExists": false
}
```

This tests recovery behavior for a stranded-backup state. It does not establish
the exact power-loss interleaving inside Windows or physically interrupt
`ReplaceFileW`.

Recover replacement pairs before cleaning them. When the destination is absent,
restore the original backup. If recovery fails, retain the backup and temporary
copy and keep their ownership records for another attempt. Only forget a record
after recovery or cleanup succeeds. `is_staged_name` currently validates the
shape of the name; it does not return the destination, so a recovery parser or
explicit recorded destination is still needed.

## Remaining direct-write paths

The review correctly identifies three workspace mutation paths that bypass
staging: `edit`, `write` and `Snapshots::put` during undo/redo. All still call
`tokio::fs::write` on the destination. A failed write can leave it truncated.
Use the common replacement mechanism after fixing its recovery behavior,
retaining existing permissions, read checks, line-ending handling and conflict
checks.

### Additional read-before-write bypass in write

`write.rs` uses `read_to_string(...).await.ok()` to decide whether a destination
exists. Invalid UTF-8 and other read failures are treated as absence.

The headless engine accepted a write to an unread existing fixture with invalid
UTF-8, captured its old bytes, overwrote it and reported `Done`.

```json
{
  "fileOverwritten": true,
  "toolStatus": "Done"
}
```

Only `NotFound` means absence. Existing unread files must be refused regardless
of whether their bytes decode as text. Unsupported contents and read failures
must not silently become new-file writes.

## Held results on automatic delivery

The supplied example reproduces: result A is held after Stop; a later-generation
result B starts a permitted parent turn. Only B enters that request, while A
stays held and undelivered. A is delivered once when a subsequent user prompt
arrives.

This follows the earlier requested user-prompt policy. Carrying held results
with any later permitted prompt is a useful lower-priority behavior change,
not a loss of their stored text. If adopted, acknowledge A and B with their
single durable parent attachment and preserve the rule that A cannot itself
wake a stopped parent.

## Database overhead

The source supports the overhead concern, but no contention benchmark was run.
The count is understated: `record_staged` executes one autocommitted insert per
path, usually two; successful replacement then forgets the temporary and
backup names separately. That is normally four inventory statements/commits.

Batch registration before filesystem changes, and batch successful cleanup
afterward, in separate short transactions. Do not keep the shared database
lock across asynchronous file operations or register files only after creating
them. Preserve records for incomplete recovery.

## Verification

- Engine tests: 322 passed, one ignored benchmark.
- Engine Clippy, all targets, warnings denied: passed.
- TypeScript typecheck: passed.
- HTTP/WS conformance: eight passed, 49 assertions.

Probe source:
`C:\Users\KYLEPE~1\AppData\Local\Temp\opencode\drift-error-body-probe\src\recovery.rs`.
It uses public engine APIs, scripted providers and disposable file credentials.
No production credentials or paid provider calls were used.
