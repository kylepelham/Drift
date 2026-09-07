# Async question comparison

Reviewed 2026-09-05. This is a focused comparison of questions, answer delivery, turn
control, and model settings. It is not a complete audit of ChatGPT or a claim of feature
parity with its desktop app.

## Evidence boundaries

Three sources were examined separately:

- The installed `ChatGPT.exe` in IDA and the running process's module paths.
- OpenAI's public Codex source and app-server documentation on `main`.
- Drift's working tree, including the existing uncommitted async-question and Astra overlays.

The installed executable belongs to package
`OpenAI.Codex_26.818.5229.0_x64__2p2nqsd0c76g0`. Its SHA-256 is
`1788d2fc76f030bdf26b8989ead9309928df7a6cf514617187a11732217014b5`.
IDA function `0x1400358A0` locates `chrome.dll`, loads it with `LoadLibraryExW`,
resolves `ChromeMain`, and calls that export. The process module list confirms
`chrome.dll` and `chrome_elf.dll` beside the executable.

This establishes a Chromium startup path. It does not identify the app's question
implementation. Listing the protected installation directory failed, and the larger DLL
and client resources were not analyzed. No installed binary was patched, no installation
permissions were changed, and no account data or network sessions were inspected.

Public Codex `main` is independently useful evidence, but it is not proof that this
installed ChatGPT build uses the same implementation or enables every documented feature.

## How public Codex async questions work

The public [async input handler][async-handler] exposes a nonblocking question tool.
It validates that questions and titles are nonempty and that supplied options contain
nonempty answers. It emits a completed agent-message item containing structured questions
and an async delivery marker, then returns an acceptance result without waiting for input.

The [app-server contract][app-server] describes these async agent messages as visible
without ending the turn. Replies arrive as ordinary user messages. The acceptance result
is not an answer or approval. Suggested/preselected choices are not submitted automatically.

There is also a distinct `item/tool/requestUserInput` server-request path. It carries
`isBlocking`, and `serverRequest/resolved` reports both user resolution and lifecycle
cleanup. Do not conflate this request protocol with async agent-message delivery or
permission approval.

## What Drift already has

Drift's [async-question overlay][async-overlay] already implements the important
nonblocking pattern:

1. Register a question and return a pending request ID immediately.
2. Continue only work that does not depend on the answer.
3. Save accepted answers as ordinary user messages, retaining the current model, agent,
   reasoning variant, custom instructions, and output format.
4. Request serialized continuation without interrupting active work.

Direct blocking questions, plan exit, and permission approvals retain their blocking
paths. Async cards support answering later, multiple pending requests, cross-workspace
ownership, and independent answer drafts. These were existing capabilities, not added by
this comparison.

The public Codex design therefore supports Drift's current direction. There is no evidence
here that a hidden model setting is required to make async questions work.

## Improvements implemented

| Problem | Change | Verification |
| --- | --- | --- |
| Repeated submissions could send duplicate or conflicting requests. | A session/request-keyed in-flight guard joins identical submissions and rejects conflicting payloads locally. | `tests/question-reply.test.ts` |
| Question reply transport could wait indefinitely. | Replies and rejections use an eight-second abort deadline. Unconfirmed requests stay visible. | Timeout, failure, retry, and owning-workspace tests. |
| A failed async submission allowed editing even though the engine required the original answer on retry. | Copy and retain the first answer; lock editing and explicitly offer retry of that original answer. | `tests/question-drafts.test.ts` |
| Switching cards lost sending/failure state. | Keep submission state keyed to the request outside the mounted card. | Shared-state lifecycle tests and component wiring checks. |
| Late transport results could outlive authoritative resolution. | Ignore completions belonging to cleared or replaced submission state. | Late success, failure, and replacement tests. |
| Rejection notification failure could leave a blocking question waiting forever. | Log notification failure and still settle the blocking caller. | Effect listener-defect and interruption regressions in the overlay. |
| Failed publication of a blocking question could leak its pending entry. | Put publication inside the existing lifecycle cleanup guard. | Pending cleanup and subsequent-question regressions. |
| Existing Astra regressions were absent from the curated engine runner. | Run the catalog fallback and Codex OAuth context-limit tests. | Both filters matched and passed in `test:engine`. |

These changes do not provide durable question recovery or exactly-once delivery across
process restarts. An aborted HTTP request does not roll back a server-side save. Dismissing
an unconfirmed request does not undo an answer that was already saved.

## Remaining priorities

The capabilities below are verified in the public app-server contract, not in the
installed ChatGPT UI. Several are explicitly experimental upstream.

| Priority | Public capability or reliability requirement | Drift comparison and next step |
| --- | --- | --- |
| 1 | Explicit interruption and turn ownership. | Drift has cancellation-generation checks in its engine, but the frontend orchestrator independently reacts to idle transitions. Test rejection/Stop across both mechanisms and give automatic continuation an explicit paused reason. This remains an integration risk, not a reproduced regression. |
| 2 | Durable user-turn queue, with stable submission identity, edit/reorder/start operations, and pause after interruption. | No equivalent user-turn queue was found in the reviewed frontend. Composer history is recall history, not pending work. Design a backend-owned queue rather than another frontend timer. The public correlation IDs alone do not prove retry idempotency. |
| 3 | Exact active-turn steering through `expectedTurnId`, with an accepting turn ID in the response. | Drift's `send` and generated `steer` both use `promptAsync`; the frontend does not send an expected active-turn ID. Separate same-turn input from idle continuation and reject stale steering at the engine boundary. Existing retry-model switching already has a separate message-ID guard. |
| 4 | Persisted goals with status and budget/accounting. | Drift derives its goal from transcript messages and keeps its 30-round continuation accounting in frontend memory. Persist goal identity, pause state, and accounting if goals must survive reloads. |
| 5 | Model-advertised reasoning effort order and service-tier metadata. | Drift already has reasoning variants and an Astra fallback with a fast mode. Prefer authoritative provider metadata when available; keep reasoning effort distinct from service speed/tier and distinguish temporary turn overrides from saved defaults. |
| 6 | Explicit request-resolution lifecycle. | Drift handles question reply/reject events and reconciles pending asks, but accepts V2 question events while replies still use the legacy endpoint. Carry protocol identity through storage, polling, and reply routing before enabling V2-originated questions. |

Two additional Drift reliability gaps deserve separate work:

- Pending questions and queued answer wakes are instance-local. Persisting a card alone
  will not restore its callback or resume work after a crash. Define recovery, expiration,
  cancellation, and answer-admission semantics before adding storage.
- Ordinary `promptAsync` transport success precedes asynchronous engine admission. Keep
  a recoverable submitted prompt until its matching user-message event appears, or add a
  durable admission acknowledgement. Do not label transport acceptance as a confirmed save.

## Making Astra more effective

The fixes above reduce lost input, misleading retries, and stalled question waits. They
do not change model weights or establish that Astra performs better on coding tasks.

The existing Astra fallback's limits, pricing, and reasoning levels were not independently
validated against a live provider and were not changed. Its tests verify Drift's configured
behavior, including deferring to native catalog metadata, rather than provider availability.

Next, add offline prepared-request tests covering default/explicit reasoning effort,
fast mode, model changes, and context limits. Verify async answers retain the intended
instructions and settings across compaction and steering. Compare task completion,
failed tool calls, repeated work, latency, and cost on the same small task set before
claiming a model-quality improvement. No live paid model evaluations were run here.

## Validation and limits

- Root test suite: 484 passed, 0 failed across 41 files.
- Root TypeScript check passed.
- Production frontend/extension build passed. The build reported CSS optimizer warnings
  for `::highlight` selectors and warnings about chunks over 500 kB; neither blocked it.
- Curated engine suite: 190 passed, 0 failed across 24 test invocations, including the
  question regressions and both Astra selections.
- Engine `opencode`, `core`, and `schema` typechecks passed.
- The vendored upstream snapshot was restored clean after engine tests.
- Question card switching was tested through shared state and component wiring, not a
  mounted browser/Tauri end-to-end test.
- Native packaging and installed-app replacement were not performed.

[async-handler]: https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/request_user_input_async.rs
[app-server]: https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md
[async-overlay]: ../engine/overlays/zzzz-async-question.patch
