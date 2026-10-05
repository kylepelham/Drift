# Claude Code binary comparison

Inspection date: 2026-09-29.

Drift has concrete weaknesses in tool discovery, concurrency control, and context
accounting compared with the installed Claude Code implementation. This inspection
does not establish a task-success score or an end-to-end speed difference.

## Artifacts and method

- Claude Code: `C:\Users\KylePelham\.local\bin\claude.exe`, reporting `2.1.85`.
- Size: 237,718,176 bytes.
- SHA-256: `4a336dc188dc44289c801a33ba1868fa2b2b39d345e9a3316327218048c91669`.
- Drift commit: `472edb81fcb8629c76ebff5cada14cb8add45d16`.
- Vendored OpenCode commit: `7f964bbb00e505178847e2c08721b0fff56208f9`.

The executable contains readable minified JavaScript as well as native code and
Bun runtime material. A temporary Node script read the binary as Latin-1 and
searched it for identifiers, then printed surrounding code and byte offsets.
Latin-1 preserves byte offsets. This was static inspection of embedded source,
not recovery of the original TypeScript or proof that every branch runs.

Offsets below identify matches in this exact executable. Search by identifier for
other builds. The inspection used `claude --version`, but did not submit model
requests, inspect credentials, or benchmark a live Claude Code session. Drift
findings describe the checked-out source and build overlays, not a verified match
to any already-running Drift sidecar.

Current public documentation corroborates some mechanisms, but can describe a
newer release. Version-specific statements below come from the local executable.

## 1. Tool discovery imposes avoidable costs

Confidence: high for the implementation difference. Net latency and task-success
effects require measurement.

Claude Code's `Z5f` function at byte 126981148 implements tool-name matching,
prefix matching, required terms, and weighted keyword search over names, hints,
and descriptions. This ranking happens locally. Its ToolSearch implementation at
126983116 supports exact `select:` requests and defaults to five results. The
tool description at 125387250 explains how matching schemas become available.
The request builder also supports `defer_loading` at 131601912.

Drift's direct-MCP path builds the connected tool catalog in
`engine/upstream/packages/opencode/src/session/tools.ts:388-397` and filters it by
permissions and user settings in `session/llm/request.ts:208-214`. Optional Jev
routing adds another filtering stage:

- `engine/opencode/tool-routing.ts:117-136` makes an external classifier request,
  with a 1,200 ms abort timer around fetch and response processing.
- Lines 152-171 cache and await the decision per session, user turn, and catalog.
- Lines 172-184 retain everything on an unsuccessful decision and offer only
  full-catalog expansion when tools were hidden.
- Lines 34-45 and 85-91 select whole MCP servers, rather than individual tools.
- `engine/overlays/zzzzzzz-jev-tool-routing.patch:18-39` places this wait before
  provider execution.

The result is a poor tradeoff for some workloads. Routing disabled means a large
upfront catalog. Routing enabled adds a network dependency before inference and
can still fall back to the large catalog. A missed selection requires a model
tool call that restores everything, rather than retrieving the needed schema.

Jev is off by default. It bypasses routing without a usable key, with fewer than
two groups, with more than 24 groups, or with a catalog exceeding 96,000
characters. Those bypasses do not incur the classifier fetch. Code mode bypasses
direct MCP exposure and must be evaluated separately.

Recommendation: add local, per-tool deferred discovery with exact-name lookup,
compact searchable metadata, and selective expansion. Measure whether any
optional classifier improves enough tasks to justify its request latency.
Claude's discovery still takes a model tool round trip when needed; local search
does not make that round trip free.

## 2. Routing can undermine prompt caching

Confidence: high that routing changes the tool set; provider-specific cache cost
is unmeasured.

The router's key includes the user turn. A later turn can receive a different
tool set, and expansion replaces a shortlist with the full set. Stable sorting
does not preserve a prefix when its contents change.

Anthropic's current [prompt caching documentation](https://code.claude.com/docs/en/prompt-caching)
explains that changing upfront tool definitions invalidates cached content, while
deferred tool changes can preserve the existing prefix. Claude Code 2.1.85 has
deferred-schema support and explicit cache-control construction. At byte
131608830, `qF` and `UO1` also select a one-hour TTL for eligible requests. Those
eligibility and feature gates are not evidence that every user gets this TTL.

Drift already uses prompt caching. OpenCode's
`engine/upstream/packages/opencode/src/provider/transform.ts:358-406` applies
ephemeral cache markers, and `session/llm/request.ts:184` sorts tools. The weakness
is changing cache-sensitive inputs without demonstrated net savings.

Recommendation: keep upfront definitions stable, retrieve deferred schemas
selectively, and record cache-read, cache-write, and uncached input tokens alongside
latency. Fewer schema tokens alone is an insufficient success metric.

## 3. Tool concurrency lacks Claude Code's safety-aware scheduling

Confidence: high on Drift's default AI SDK direct-tool path. Actual race frequency
has not been measured.

At byte 129635372 Claude Code validates input, calls `isConcurrencySafe`, and
queues execution. `canExecuteTool` permits overlap only when the incoming tool and
all executing tools are marked concurrency-safe. The non-streaming grouping path
at 129644373 uses the same distinction. MCP tool construction at 127478065 derives
the flag from `annotations.readOnlyHint`, defaulting to false.

Drift delegates default execution to `streamText` in
`engine/upstream/packages/opencode/src/session/llm.ts:276-280`. The installed AI SDK
6.0.168 implementation in
`engine/upstream/packages/opencode/node_modules/ai/src/generate-text/run-tools-transformation.ts:328-347`
starts tool execution without awaiting completion so the stream can continue.
Drift's MCP conversion in `engine/upstream/packages/opencode/src/mcp/catalog.ts:42-82`
does not carry the MCP read-only annotation into a scheduling decision. The
execution wrapper in `session/tools.ts:398-419` checks permission but does not add
a shared-state scheduling barrier.

Parallel reads are useful. Overlapping stateful calls can be wrong: switching a
desktop target while another call uses it, navigating a shared browser session
while another call reads it, or invoking dependent mutations concurrently.
Instructions asking the model to parallelize only independent work do not enforce
this at runtime.

Recommendation: introduce execution metadata and a scheduler. Serialize tools
with unknown or mutating behavior by default; permit verified safe operations to
overlap. Resource-specific locks can later allow unrelated stateful operations
to run together. MCP annotations are hints, so local overrides remain necessary.

This is a reliability finding, not a claim that Drift lacks parallel execution.
The native opt-in runtime and code-mode execution need separate scheduling tests.

## 4. Drift's context breakdown can blame the wrong inputs

Confidence: high. The mismatch is visible in source.

Claude Code's context analyzer at bytes 129885701 and 129891576 separately accounts
for prompt sections, memory files, built-in tools, MCP tools, deferred tools,
agents, skills, and messages. This proves component-specific accounting exists;
it does not establish perfect tokenizer accuracy on every provider.

Drift's `src/engine/context-breakdown.ts` instead:

- Estimates text at four characters per token, line 8.
- Estimates arguments by key count times 16 characters, line 22.
- Counts full completed tool output without checking `time.compacted`, line 23.
- Drops everything before the latest assistant summary, lines 29-34.
- Calls the unexplained remainder system prompt and tools, lines 52-67.

The engine's actual request representation differs. In
`engine/upstream/packages/opencode/src/session/message-v2.ts:297-300`, pruned
outputs become a short placeholder and lose attachments. Lines 525-575 preserve
and reorder a retained history tail that can precede the summary in stored
transcript order. The UI heuristic misses both distinctions. Partial transcript
hydration introduces another source of missing category data.

Consequently, the category bar can overstate tool results, omit retained messages,
or assign missing content to system/tools. Its total comes from reported usage;
the finding concerns attribution, not fabrication of that total.

Recommendation: compute categories from the engine's prepared request and expose
them to the UI. Keep estimates explicitly labeled and account for deferred schemas,
pruned results, retained history, and media separately.

## 5. Performance diagnosis is less useful in Drift

Confidence: high for the diagnostic implementation found; completeness of all
possible external tracing configurations was not assessed.

Claude Code's timing formatter at byte 129642089 separates pre-request overhead
from request-to-first-chunk time. Its phase table includes context loading,
microcompaction, autocompaction, query setup, schema construction, message
normalization, client creation, and tool execution. Its label says TTFT, although
the observed endpoint is the first received chunk, which need not be visible text.

Drift has startup milestones, tool timing data, and optional OpenTelemetry in
`engine/upstream/packages/opencode/src/session/llm.ts:208-222,344-352`. It is not
un-instrumented. However, `src/engine/bench.ts` seeds a synthetic transcript rather
than measuring provider-turn latency, and the inspected UI has no equivalent
joined breakdown from send through preparation, provider response, and paint.

`docs/engine.md:192-210` records a reverted preparation-overlap experiment and
explicitly distinguishes reactive-update tests from end-to-end speed. Its Jev
section also says task-success and latency benefits are unbenchmarked.

Recommendation: connect existing spans to a per-turn diagnostic record with
admission time, preparation phases, request sent, first event, first visible text,
tool waits, completion, and cache usage. Optimize the phase that measurements show
is expensive.

## Comparisons the evidence does not support

- No numeric claim that Claude Code starts faster, completes tasks faster, or
  writes more correct code. This requires repeated matched-task runs with model,
  reasoning settings, provider, repository, tools, and cache state controlled.
- No claim that Drift lacks compaction, pruning, prompt caching, parallel tools,
  subagents, or revert. Those capabilities already exist.
- No claim that Claude's time-based microcompaction is universally active. At byte
  127028594 its fallback configuration has `enabled: false`, a 60-minute gap, and
  five recent results retained. A remote feature gate can change that.
- No claim that Tauri, Solid, SQLite, or the sidecar architecture explains a speed
  difference. Drift's 300 ms readiness polling can add detection delay, but it is
  not a measured startup comparison.
- No inference that minified feature branches are enabled for this account, or
  that the binary reveals server-side inference optimizations.

## Work order and verification

First add per-turn measurements, then implement cache-stable tool discovery and
safety-aware execution scheduling. Correct context attribution alongside that work.
Compare routing off, Jev routing, and deferred discovery on the same task corpus.
Include many-tool tasks, shared-state automation, long conversations, and resumption
after compaction. Track completed-task correctness, tool errors, median and p95
latency, provider calls, and cache usage.

Repository validation for this initial inspection: `bun run typecheck` passed.
No live task-quality benchmark was run. At the end of the initial pass this report
was the only repository change; the later [research dossier](research/README.md)
adds deeper traces, reproducible inspection scripts and a separate proposed
revision of the clean-room branch plan. The initial temporary helper remains
outside the repository.
