# Claude Code 2.1.85 execution loop: static reconstruction

Research date: 2026-09-29. This is a design input for a Drift-owned engine, not an implementation specification copied from Claude Code. The executable is `C:\Users\KylePelham\.local\bin\claude.exe`, 237,718,176 bytes, SHA-256 `4a336dc188dc44289c801a33ba1868fa2b2b39d345e9a3316327218048c91669`. It reports version 2.1.85. Byte offsets below address Latin-1-decoded embedded JavaScript in that exact executable, so the offsets are also binary file offsets. Identifiers such as `dcf` are minifier products and will change on rebuild. I inspected bounded excerpts around functions and their callers; I did not invoke a model, read credentials, or deposit the executable or bulk source in this repository.

**Confidence key.** High means the branch and its caller are visible in this binary. Conditional means the path depends on an observed gate, configuration, environment, or runtime error. Unknown means static code cannot establish the live outcome. This account's remote flags were not queried. The preceding [binary comparison](../claude-code-binary-comparison.md) establishes context for tool discovery and timing, but this report follows execution and recovery rather than repeating its comparisons.

## Control flow and ownership

The `ab` wrapper delegates to `dcf` and marks accumulated prompt events complete on return. `dcf` is a loop over a small explicit state record, `w`, rather than a recursive call per model turn. It retains messages, tool context, turn number, auto-compaction tracking, output-limit recovery count, reactive-compaction attempt, pending tool summary, stop-hook state, and transition reason. Dependencies (`callModel`, `microcompact`, `autocompact`, `uuid`) can be injected; default `callModel` is `evH`, a wrapper over `LC6` and `xN_` [129665350-129666000; 131611361-131611780].

```text
ab -> dcf loop
  prepare history -> microcompact -> autocompact -> setup scheduler/model
  -> callModel async iterator
       assistant event -> expose message, collect tool_use blocks
       optional streaming scheduler -> dispatch tools, emit progress/results
       streaming/provider fallback -> reset provisional messages and scheduler
  -> aborted? finish/cancel tools and exit
  -> no tools? reactive recovery / output-limit recovery / Stop hooks / exit
  -> tools? drain scheduler (or grouped fallback), attach context, next turn
```

Evidence: `dcf` constructs `w`, does `Z5K`, `microcompact`, `autocompact`, then creates `new SH8` only if `p5("tengu_streaming_tool_execution2")`. It calls `M.callModel(...)`, accumulates `PH` assistant messages and `EH` tool-use blocks, then either drains `l.getRemainingResults()` or calls `_F8(EH,PH,...)`. The next `w` contains `[...F,...PH,...MH]` and transition `next_turn` [129665400-129668000; 129674900-129677800]. `C4_` tests the remote gate for streaming, an environment switch for summaries, and defaults `fastModeEnabled` unless explicitly disabled [129665040]. None of these prove a specific account runs streaming tool execution.

| Transition from `dcf` | Trigger | Effect and evidence |
| --- | --- | --- |
| `next_turn` | Tool blocks completed, attachments incorporated | Refreshes tools if requested, appends assistant and tool results, increments turn; `maxTurns` exits before the next turn [129676100-129677800]. |
| `stop_hook_blocking` | Stop hook returns blocking text | Adds meta user messages to prompt and reruns model with `stopHookActive=true`; `preventContinuation` instead exits [129675100-129675900; 129660392]. |
| `max_output_tokens_recovery` | Output-limit API error and recovery count below `Fcf=3` | Adds a meta user instruction to continue, clears output override, retries; at limit yields the error [129675350-129675900; 129677850]. |
| `reactive_compact_retry` | Withheld long-prompt or media-size error and reactive component succeeds | Replaces prompt with compact result, records attempt, retries; otherwise exits with `prompt_too_long` or `image_error` [129674600-129675500]. Conditional on `tOH` and its gate. |
| `aborted_streaming`, `aborted_tools` | Parent signal aborted at phase boundary | Drains/synthesizes outstanding results; runs computer-use cleanup for main agent, then exits [129674200-129674800; 129675900-129676200]. |
| `model_error`, `image_error` | Provider throws after local fallback handling | Synthesizes error results for tool uses already seen, emits API error and exits [129673900-129674400]. |

The loop runs `BLK([...F,...PH],...)` when it has assistant messages before checking abort, but the meaning and durability of `BLK` were not established here [129674150]. Do not interpret a generated assistant event as a committed durable turn.

## Dispatch and concurrency

`yH8` locates the tool by name with an alias fallback, then `Xcf` couples `Zcf` to a progress queue. `Zcf` parses input with the tool schema, calls optional `validateInput`, executes PreToolUse hooks, resolves permission, invokes `H.call`, maps the result, runs PostToolUse hooks, and emits user `tool_result` or error. Unknown tool, malformed input, validation failure, denial, cancellation, and thrown execution error each produce a linked `tool_use_id` result instead of silently losing the call [129619740-129623900; 129626000-129633300]. The `B8` constructor creates a user message with UUID, timestamp and `sourceToolAssistantUUID` [131538096].

### Streaming scheduler `SH8`

| Stored state | What changes it | Output rule |
| --- | --- | --- |
| `queued` | `addTool` after input parse and `isConcurrencySafe(parsed)` | `processQueue` scans in tool order; starts safe calls alongside *only* safe executing calls; stops at an unsafe queued call it cannot run [129635500-129636380]. |
| `executing` | `executeTool` starts `yH8` with child abort controller | Progress enters `pendingProgress`; final messages and context modifiers accumulate privately [129637000-129639200]. |
| `completed` | Iterator finishes, or abort yields synthetic error | `getCompletedResults` emits progress then results and removes in-progress tool ID; for an executing unsafe call, it stops walking later tools [129639200-129640150]. |
| `yielded` | Results emitted | `getRemainingResults` waits on executing promises or progress and keeps starting queued calls until all are yielded [129639500-129640700]. |
| `discarded` | Streaming fallback or model fallback | Generator stops yielding. It does **not** visibly undo a tool's external side effects [129636450; 129672400-129674000]. |

`canExecuteTool(safe)` is `executing.length===0 || safe && executing.every(safe)` [129636050]. Thus an unsafe call is an exclusive barrier while executing. The scheduler tests the tool's `isConcurrencySafe` **only after schema parse**; an invalid input defaults to unsafe [129635550]. Safe context modifiers are buffered and applied in tool order after the safe group in `_F8`, while streaming `SH8` only applies modifiers immediately for unsafe calls [129644400-129645700; 129638900]. This is a meaningful design asymmetry: the visible streaming class does not apply modifiers returned by safe tools to its shared context. Investigate whether such tools are constrained not to return modifiers before copying that behavior.

The non-streaming `_F8` path groups consecutive safe calls with `kcf`, executes each group via `np8` at a cap of `parseInt(CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY)||10`, then runs unsafe calls serially [129644373-129645750; 128779886]. The cap is visible in the non-streaming group path; there is no equivalent cap in the inspected `SH8.processQueue`. It would be wrong to say the streaming scheduler is capped at ten. `np8` races async generators, so progress/results from safe tools can interleave [128779886]. The prior comparison's observation that Claude has safety-aware scheduling is correct, with this cap qualification.

The streaming scheduler has an `hasErrored` sibling-abort branch only when an error `tool_result` comes from a specific tool identifier `jq` (the shell tool in adjacent parameter handling). An ordinary failed read or MCP call does not visibly trigger global sibling cancellation [129637400-129638800]. `processQueue` stores `H.promise` and attaches `.finally(()=>this.processQueue())` without an explicit `.catch` at that site; the surrounding `yH8` catches tool exceptions, but this alone does not prove all scheduler failures are handled. No automatic tool-call retry is visible in `yH8`/`Zcf`; a tool's own implementation may retry internally [129619740-129633300].

### Hook and permission ordering

```text
schema parse -> validateInput -> PreToolUse -> resolve permission
             -> tool.call -> map output -> PostToolUse
             \-> thrown tool error -> PostToolUseFailure
no tool blocks -> Stop / SubagentStop -> optional blocking feedback -> model again
```

`f4_` translates PreToolUse outputs into deny, allow, ask, input update, context attachment, and stop reason [129615636-129618400]. `Zcf` does **not** treat hook allow as an unconditional bypass. For a noninteractive tool, it checks `st6` rules first; a deny overrides the hook, an ask prompts, and only a null result allows the hook decision. `requiresUserInteraction` and `requireCanUseTool` can still force `canUseTool`. A hook deny produces a failed result. An updated input is used for permission and execution; code at this site does not visibly re-run `inputSchema.safeParse` after an arbitrary hook edit [129624000-129630000]. That is a validation question for Drift, not proof of an exploitable path. Permission decisions are classified by source (`session`, settings, hook, rule, mode, prompt) for telemetry [129619200].

After a successful call, `K4_` runs PostToolUse, reports blocking errors and stop-continuation attachments, and for MCP tools can replace the output before result mapping. On a thrown call `_4_` runs PostToolUseFailure and emits its context/errors after the failed result. Hook execution errors become attachments rather than unhandled tool exceptions [129612440-129616400; 129630300-129633300]. `y4_` calls Stop/SubagentStop hooks after a model response with no tools; blocking feedback is fed to the next prompt, while `preventContinuation` exits. It also conditionally runs TaskCompleted and TeammateIdle checks after Stop [129660392-129664000]. These are observed call sites, not a claim that every installed hook type is enabled for this user.

## Abort, fallback and retries

`rR(parent)` propagates the parent abort reason to a child signal and removes listeners after child abort [125069930]. `SH8` owns a sibling controller derived from the turn controller; each executing tool gets another child. Its abort-reason priority is discarded, sibling error, then user abort. An interrupt with `interruptBehavior()==="block"` returns no synthetic cancellation from `getAbortReason`, whereas `"cancel"` produces user rejection; other parent abort reasons reject. Progress already received can be emitted before completion [129636400-129639900]. Treat abort as a request to stop, not a rollback of side effects.

`jy8` is the **provider request** retry loop. Defaults are ten retries, initial exponential delay 500 ms with jitter, and a general backoff cap 32 seconds; it checks abort before attempts and during waits. Status, headers and provider mode affect retry eligibility. It can refresh client/auth, adjust `max_tokens` on a particular context-limit 400, disable fast mode after some errors, and throw `e3H` after consecutive overload errors when a fallback model exists [126954323-126959700]. Those rules do not turn a failed `H.call` into an automatic tool retry. `IN_` invokes the same `jy8` for non-streaming `beta.messages.create`, explicitly creating an SDK client with `maxRetries:0`; the wrapper owns the retries [131611800-131612500].

Streaming failure inside `xN_` can switch to non-streaming unless disabled by environment or remote flag; stream-creation 404 has a separate fallback path. A user abort is rethrown rather than substituted with a non-streaming request [131625400-131627900]. `xN_` calls `onStreamingFallback` **before** yielding replacement assistant content. In `dcf`, that callback sets a flag; on the next yielded item, `dcf` tombstones assistant events in `PH`, clears assistant/tool/result arrays, discards the old scheduler, and creates a fresh one. Separately, `dcf` catches overload fallback `e3H`, emits error results for prior tool-use blocks, clears provisional state, changes `mainLoopModel`, and repeats the request [129672000-129674200]. The tombstone branch concerns locally emitted messages and pending dispatch, not an idempotent rollback of tools already begun. A replacement engine should avoid early side effects, or give them explicit transaction/compensation semantics.

## Messages, persistence and restart

`p2` converts stored events into API messages: it drops progress, merges adjacent user messages, joins assistant fragments by message ID, and normalizes tool and attachment blocks [131550270-131553000]. The model request is thus a projection of stored messages, not a byte-for-byte replay. Assistant tool blocks and user tool results carry distinct IDs and the latter links back via `sourceToolAssistantUUID` [131538096; 131440693]. `HB6` filters assistant messages with tool uses lacking matching result IDs from a forked subagent's inherited context; it prevents replay of dangling calls in that path, but does not prove general foreground crash recovery does the same [128796094].

Persistence is an append queue, not a commit before every visible event. `insertMessageChain` attaches `parentUuid`, `logicalParentUuid` for compact boundaries, `isSidechain`, `agentId`, and session metadata; non-sidechain UUIDs are deduplicated through `FSH`. `appendEntry` queues JSON lines. The queue flush timer defaults to 100 ms, `flush()` waits for pending writes, and entries can be skipped under test or session settings [131436506-131438300; 131440693-131445000]. The foreground SDK caller invokes `sk(_H)` without awaiting on assistant/progress paths but awaits it on user messages; an eager-flush or cowork option explicitly calls `Id()` near final success [132944449-132946000]. The subagent `Tk` awaits `wd` for initial and subsequent sidechain events, but catches and logs write failures [128795022-128796094]. Do not promise that the last displayed token survives an abrupt process death.

Remote ingestion and internal-event persistence are optional branches. Foreground or subagent internal readers can hydrate JSON-line transcripts, and a special epoch mismatch is rethrown during one hydration path [131443000-131447000]. This is evidence of resume support, not proof of exactly-once tool execution after a crash. The foreground SDK consumer ignores `tombstone` events in its switch while it may have queued an assistant write earlier; the visible code does not establish that all provisional persisted assistant records are removed on streaming fallback [132945571; 129672000-129673200]. Verify this with an actual interrupted session before assuming a clean replay.

## Subagent boundary

The later [async-worker investigation](claude-async-workers.md) extends this with
ordinary launch-mode selection, compiled-off fork gates, skill routing, background
notifications and the pending M3 implementation contract.

`Tk` creates a new agent ID, chooses model and tools, builds inherited or new context, runs `SubagentStart`, loads optional agent skills and MCP clients, then builds an agent-specific `toolUseContext` and calls the same `ab` loop. It records initial context and sidechain events separately. Its `finally` closes dynamic clients, removes hooks, clears read state and releases task state. `maxTurns` breaks its iterator; abort throws after iteration [128791059-128796100]. An async agent normally gets its own controller; a foreground child normally shares the parent's controller unless an override supplies one. Its permission context may avoid prompts when async, inherit explicit allowed rules, or use agent-specific permission mode; it does not implicitly inherit unlimited authority [128792000-128794400]. `E0` is a separate fork runner used by background tasks such as memory extraction, with optional transcript suppression; these two launch paths should not be conflated [129689559-129690300].

Subagent progress is filtered through `FIf`; only assistant, user, progress, and compact-boundary system events pass that path [128791059]. For parent resume, subagent event readers are separately registered and hydrated [131443000-131447000]. That shows durable sidechain records exist; it does not establish whether an interrupted async worker itself resumes running. The partial orphan-call filter `HB6` argues for explicit reconciliation on resumption.

## Replacement rules for Drift

These are proposed algorithms, not claims that Claude Code already implements them.

1. **Turn state as an append-only journal.** Persist immutable `turn_started`, `request_attempt`, `assistant_delta`, `assistant_final`, `tool_queued`, `tool_started`, `tool_finished`, `turn_finished` events with stable session, turn, attempt and tool-use IDs. Mark streamed text provisional until its provider attempt completes. On fallback, invalidate the attempt and generate a new one; do not reuse a provisional tool-use ID for a fresh response.
2. **Commit before dispatch.** Validate full tool input and the post-hook edited input, resolve permission, then durably record a dispatch intent. For a tool with external side effects, persist execution status before invocation and completion afterward. On restart, reconcile a started-but-unfinished operation instead of blindly rerunning it. Require idempotency keys where the tool supports them; for non-idempotent operations, ask or inspect state.
3. **Bounded resource scheduler.** FIFO queue with explicit `safe`/`unsafe` and resource keys. Default unknown tools to exclusive. Permit overlapping safe calls only when all active calls and resource locks allow it. Bound concurrency on *both* streaming and batch paths; preserve request order for final result assembly while sending progress promptly. Apply context modifiers in a defined order even when safe tools finish out of order.
4. **Single permission transaction.** Normalize and validate input, run PreToolUse, validate any changed input, check hard denies, resolve asks, then execute. Record the decision source and effective input. PostToolUse changes output/context, never silently erases a completed external mutation. Feed Stop feedback back to the model with a bounded loop and a visible stop reason.
5. **Abort tree with terminal accounting.** Parent turn -> request, each tool, each subagent. Interruptible tools receive abort; blocking tools finish or reach a declared reconciliation deadline. Record one terminal disposition for every committed tool use, including `outcome_unknown` when its external effect cannot be established. Preserve the difference between a user interrupt, provider timeout, sibling error and fallback invalidation. A disposition is not proof the external operation stopped.
6. **Separate retry policies.** Provider retries only before a finalized provider response, using abortable backoff and request IDs. Tool retry must be declared per tool, idempotency-aware, and never inferred from a provider 429/529. Non-streaming fallback opens a new attempt; any speculative side effect on the previous attempt is an explicit conflict to reconcile.
7. **Subagents as child jobs.** Persist parent/child identity, allowed tools, permission mode, independent abort policy, transcript cursor, and completion state. Recover or mark interrupted jobs explicitly. A child result inserted into its parent should carry the child job ID so replay never duplicates a tool result.

## Failure drills and validation

| Drill | Expected replacement behavior |
| --- | --- |
| Two safe reads, then a write, then a read | Reads may overlap, the write waits for both, and the final read waits for the write. Verify result order and max concurrency. |
| Invalid tool input and hook-edited invalid input | Neither reaches `tool.call`; each emits one linked error result. |
| Hook allow meets explicit deny, hook ask meets required prompt | Deny wins; required prompt remains. Decision records name the rule/hook. |
| PostToolUse blocks after a successful write | The write stays recorded as executed; continuation stops with an explicit hook outcome. |
| Shell tool fails while safe siblings execute | Cancellation has one terminal result per sibling; no accidental result from the abandoned provider attempt enters the next prompt. |
| User interrupts a blocking tool and an interruptible tool | Interruptible tool aborts; blocking tool follows its declared deadline. Both leave terminal records. |
| Streaming response starts a tool, then provider stream fails | Replacement request gets a new attempt; verify whether prior side effect happened and avoid a duplicate mutation. |
| Process dies between tool start and result persistence | Startup detects uncertain state and reconciles without automatic replay of the mutation. |
| Provider 429 then success, versus tool process error | Provider alone retries with backoff; tool error remains a tool result unless the tool declares safe retry. |
| Stop hook repeatedly blocks | Bounded corrective turns end with a visible reason rather than looping forever. |
| Child killed mid-work and parent resumed | Parent sees child as interrupted or recoverable, with no dangling tool-use block inserted into API history. |

## Boundaries of this inspection

- Static presence is not runtime activation. Streaming execution, dynamic tool loading, fast mode, reactive compaction, remote resume, and some hook types are gated or configured [129665040; 131613000; 129674600; 131443000].
- I did not establish Claude's actual crash durability, whether `SH8` has an external concurrency cap, whether provider fallback always follows the same path across gateways, or whether safe tools can produce context modifiers. These need controlled local runs without paid calls where possible, and instrumented integration tests in Drift for the replacement.
- The prior comparison's scheduler description should be amended to distinguish the observed batch cap of ten from the inspected streaming scheduler, and to note that `discard()` suppresses later results but cannot reverse a tool that already acted [129644373; 129635500-129640700].
