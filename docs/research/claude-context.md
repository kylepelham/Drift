# Claude Code context pipeline: binary research for Drift

Inspection date: 2026-09-29. Scope: context and tool-schema preparation in
Claude Code 2.1.85, compared with the checked-out Drift/OpenCode tree. This is
research for a user-authorized replacement of the core, not an implementation
specification. See [the initial comparison](../claude-code-binary-comparison.md)
for artifact identity, other lanes, and general method.

## Evidence and limits

The inspected executable is `C:\Users\KylePelham\.local\bin\claude.exe`,
237,718,176 bytes, SHA-256
`4a336dc188dc44289c801a33ba1868fa2b2b39d345e9a3316327218048c91669`.
The offsets below are byte offsets into its Latin-1-decoded bytes, which retain
the original offsets. A temporary Node probe read bounded excerpts around
identifiers. No requests to a model, credential inspection, dynamic tracing, or
full-bundle repository copy occurred. Minified names are only useful with this
exact executable. `F8` flags and settings can change behavior remotely. An
observed branch is not proof that it ran for this account. All numeric defaults
below are implementation defaults, not measured performance results.

Evidence labels: **Observed** is executable code or checked-out source;
**Inference** connects observed paths without runtime proof; **Proposal** is
Drift design advice, not something proven by the binary.

## Pipeline and placement

```text
turn + transcript + tool catalog
  |  query loop: optional microcompact -> autocompact -> query setup
  |                (129666814)        (127036478)
  v
tool search decision -> discovered-tool history -> request schemas
  (129896739)          (129899602)          (131614600)
  |                                         |
  |                             system + message cache markers
  |                             (131602800, 131632000)
  v                                         v
provider request -> tool execution -> result persistence/budgets
                                        (125238500, 125241781)
  |                                                 |
  +----------------------- next request <------------+

autocompact: threshold -> optional saved-session-memory reuse
                        -> otherwise summary request + retry
                        -> boundary + summary + restored context
                           (127034478, 127006402, 127017285)
```

The main loop at 129666814 invokes `microcompact` then `autocompact` before
building the next request. These are separate steps with separate gates.

## Deferred tools: actual algorithm and gates

**Observed:** `tJ` at 125386769 marks MCP tools deferred, except built-in
exceptions and feature flags; other built-ins use `shouldDefer`, unless the
`tengu_defer_all_bn4` flag applies. `TH$` at 129896739 returns search mode
`tst` in its fallback path, with environment overrides for standard or auto.
`vrH` at 129898291 disables search if the model does not support
`tool_reference` blocks, the ToolSearch tool is unavailable, standard mode is
selected, or automatic selection falls below threshold. Its default unsupported
model list includes `haiku` (129899800); the list is configurable. The auto
branch estimates deferred schema tokens, subtracts a fixed 500-token allowance,
then compares against a configured percentage of the context window; when token
counting fails it compares character length against a fallback threshold
(129899800). `Ph` at 129897291 is an optimistic availability check and can
reject an unconfigured non-first-party host. Therefore neither the fallback
mode nor the existence of `defer_loading` proves universal enablement.

**Observed:** request construction at 131614600 collects discovered names from
prior user tool results containing `tool_reference` blocks and from
`preCompactDiscoveredTools` on compact boundaries (`tU` at 129899602).
When search is enabled, it includes all non-deferred tools and only discovered
deferred tools in the request. Those discovered definitions retain
`defer_loading: true` when built by `eF8` at 131600900. When search is off,
ToolSearch is removed and remaining tools are included. With search on, it
announces deferred names in a sorted `<available-deferred-tools>` meta message,
unless the `tengu_glacier_2xr` delta path is enabled. That path tracks
added/removed names in transcript attachments (129900167, 129937579).

**Observed:** `ToolSearch` at 126983116 searches the deferred catalog. Its
`select:` path splits comma-separated names, resolves exact names, deduplicates
and returns partial successes. Keyword mode (`Z5f` at 126981148) first checks
case-insensitive exact name, then MCP-name prefix, then required `+term` filters
and weighted matches on name parts, hint and dynamically obtained description.
Exact name-part hits score 12 for MCP, 10 for other tools; partial name-part
hits score 6/5; full-name substring 3 if score still zero; hint word 4;
description word 2. It drops zero-score results, sorts descending and takes
`max_results`, default 5. This local ranking need not be zero-cost: descriptions
are acquired asynchronously through each tool's `prompt` function. The prompt
description at 125387250 says matches yield full JSON schemas; the actual
tool result contains `tool_reference` blocks with names (126983116), which the
protocol resolves. The search-description cache is cleared when the sorted
deferred-name catalog changes (126980428). An unknown-name response can also
list pending MCP servers.

**Observed:** if the model tries a deferred tool whose schema was not in the
discovered set, input validation can append an explicit instruction to call
ToolSearch with `select:<name>` and retry (`Jcf`, 129621908). That is a
recovery path, not a guarantee against a failed tool call.

**Inference:** deferring whole JSON schemas can make repeated requests smaller
and more cache-stable, at the price of a model/tool-search round trip. Carrying
discovered names through compaction prevents a common post-summary regression.
It does not prove higher success rate or lower wall-clock time. The initial
comparison's phrase "ranking happens locally" is accurate about ranking,
but should not be read as "all search inputs require no async work."

**Proposal:** for a provider-portable core, store a stable searchable registry
of names, hints, descriptions and schema hashes; expose one explicit search
tool with deterministic exact/prefix/keyword selection and schema retrieval.
Track per-session discovered names and replay them across summary boundaries.
For providers without `tool_reference`/`defer_loading`, an ordinary search-tool
result can describe selected schemas, but making a newly callable tool mid-turn
requires another request with updated tool definitions. Test this capability per
provider rather than assuming Anthropic's reference-block semantics.

## Stable prefixes and prompt caching

**Observed:** `p7$` at 131602800 creates separate system text blocks. A
`tengu_system_prompt_global_cache` gate, or a force environment variable,
enables a boundary marker split into static global-scoped and dynamic uncached
text. Without the marker it falls back to an organization-scoped joined block;
some billing/special blocks receive distinct scopes. It can skip the global
block when tool definitions make that cache arrangement unsafe. `tO1` at
131634169 attaches cache controls only when prompt caching is enabled and the
block's cache scope permits it. `sO1` at 131632000 marks the last message
content block, or the penultimate message when `skipCacheWrite` applies; it
also supports a gated cache-edit path that references prior tool-use IDs. The
builder at 131615276 inserts a sorted deferred-tool name list into the message
history unless the delta path is enabled. Sorting is deterministic but changed
catalog membership still changes that message.

**Observed:** `qF` and `UO1` at 131608508 add `ttl: "1h"` to ephemeral cache
control only when the Bedrock environment override qualifies or when account
eligibility and query-source allowlist match. Fallback is an empty allowlist;
this is not a default one-hour cache for everyone. The Anthropic TypeScript SDK
defines `Tool.defer_loading`, `Tool.cache_control` and cache read/write usage
fields in [Messages types](https://github.com/anthropics/anthropic-sdk-typescript/blob/main/src/resources/messages/messages.ts).
SDK type availability is separate from availability for any particular account
or gateway.

**Observed in Drift:** `session/llm/request.ts:56-113,148-185,208-214` combines
system context and plugins, filters tools by permissions/user settings, then
sorts names. `provider/transform.ts:358-406` already applies ephemeral caching.
The Jev router changes which server schemas appear each user turn (initial
comparison, sections 1-2). It may erase a previously cached prefix even if
the shortlist is smaller.

**Proposal:** freeze ordering and version the effective static prefix by
provider/model, permissions, tool registry and system-context revision. Put
volatile per-turn material after stable blocks. Instrument actual cache read,
cache creation by TTL, uncached input, request latency, and catalog revisions.
Provider adapters should own scope/TTL/cache-control encoding. The portable
core should own deterministic inputs, cache observability and invalidation.
Do not promise that moving a dynamic tool list into a message is cache-neutral.

## Compaction, retries and recovery

| Path | Executable evidence | Gate and consequence |
| --- | --- | --- |
| Time-gap microcompact | `YLK`, `t5f`, `e5f` at 127028594-127030312 | Fallback `{enabled:false,gapThresholdMinutes:60,keepRecent:5}`. Requires eligible main-thread source and a valid last assistant timestamp. On a sufficient gap, select eligible tool-use IDs, keep last five, replace older matching user tool-result contents with `[Old tool result content cleared]`, reset cached microcompact state, record estimated tokens. No change if nothing to clear. |
| Other microcompact work | Main-loop call at 129666814 and state reset `_r` at 127028727 | The time-gap function is one path, not the entire `microcompact` subsystem. This inspection does not establish defaults or exact algorithm for every other microcompact branch. |
| Auto threshold | `HF`, `yrH`, `KYH`, `wzf`, `yLK` at 127034478-127038000 | Effective window subtracts capped output reservation (20,000), then auto threshold subtracts 13,000; optional percentage override is capped. Auto needs enabled setting and no disable env, skips `compact`/`session_memory` sources, subtracts already-freed tokens, and uses a three-consecutive-failure circuit breaker. Blocking limit is effective window minus 3,000 unless overridden. Numeric warning/error reservations are both 20,000 here. Model/window functions remain dependencies, so 13,000 is not a universal absolute threshold. |
| Summary compaction | `svH` at 127006402 | Runs pre-compact hook, obtains added instructions, invokes summary request, then constructs boundary + summary + context attachments + hook results. Persists discovered tools on boundary. Logs both API-reported and estimated post-summary counts. |
| Too-long retry | `S0K`, `C0K`, `svH` at 127005994-127014242 | On a recognizable prompt-too-long summary response, drop leading grouped message chunks while retaining a tail; by default drop about 20% of groups on a retry without a parsed token target. Up to three retry trims. If the new first message is assistant, prepend a truncation marker. Failure still surfaces when no valid summary is produced. |
| Session-memory reuse | `ny8`, `fzf`, `iy8` at 127032056-127034478 | Requires explicit enable flag or *both* remote `tengu_session_memory` and `tengu_sm_compact`; falls back to ordinary compaction on absent/empty notes, missing anchor, errors, or retained context still exceeding threshold. Selects a recent tail using 10,000 minimum estimated tokens plus five text messages, or 40,000 maximum, then backs up to preserve tool-use/result pairs. Per-section note text is truncated with a pointer to full notes. |
| Post-compact restore | `u0K`, `qr` at 127017285 and 127006402 | Clears read-file state, re-reads up to five most recently read eligible files, at most 5,000 tokens per file and 50,000 aggregate estimated tokens. Also carries plan reference, invoked skills, task status, tool/server listing deltas and hook context. The restored files are fresh reads, not blindly reused cache entries. |

**Observed in Drift:** checked-out `session/overflow.ts` calculates the local
overflow threshold; `session/compaction.ts:223-269` preserves recent turns
within a 2,000-15,000-token estimated budget, `:273-316` prunes old tool
outputs if enabled, and `:319-556` handles summary, tail selection, media
overflow replay and continuation. `message-v2.ts:525-575` projects the
summary plus retained tail. These are existing capabilities. A replacement
should preserve them, rather than assume Claude has uniquely solved long
contexts. The main differences to evaluate are resumable discovered schemas,
file re-reads, notes reuse, and explicit circuit-breaker/retry behavior.

**Proposal:** separate a lossless event transcript from a compacted request
view. Record the exact reason and revision of every omission. Preserve
user constraints, tool-call/result pairing, in-flight tasks, file identities,
and discovered schemas. Treat saved notes as an optional, explicitly stale
summary source with a fallback to fresh summarization. Limit retries and
detect repeated near-threshold summaries. Validate continuation on interrupted
tool calls and provider context-overflow errors.

## File-read state and invalidation

**Observed:** the read state is a bounded cache-like map. Its clone helper
`qx` at 125320405 preserves max entries and max size; default cache settings
nearby are 100 entries and 26,214,400 bytes. Edit validation at 129054200
requires a previous full, nonpartial read, checks the current file and
modification timestamp, allows a later timestamp only if a full cached copy
still equals current content, then checks the requested replacement is present
and unambiguous. Write validation at 129063100 likewise rejects no read or
partial view, verifies modification time and rechecks content before writing;
successful writes replace the read-state entry with the new content and
timestamp. On full compaction, `svH` at 127007700 snapshots then clears the
map and calls `u0K` to reread selected files. These are optimistic concurrency
checks, not a general guarantee that arbitrary external writers cannot race
between check and write.

**Proposal:** maintain read evidence scoped to workspace and session,
including normalized path, content hash, mtime/size, full-versus-partial
coverage and source revision. Validate again at mutation time; invalidate on
edits, external watcher events and compaction. Keep a stable file reattachment
budget and surface when a file cannot be reloaded. Test equal mtime with
changed contents, rapid edits, symlinks, partial reads, deletion and rename.

## Saved session notes and separate project memory

**Observed:** `CV1` at 132927990 initializes note-taking only once transcript
usage reaches 10,000 estimated tokens. Thereafter it requires 5,000 additional
tokens and either three tool calls since its previous anchor or a lack of the
`k88` activity condition. The main-thread hook `mV1` checks
`tengu_session_memory` before running and invokes a restricted fork whose only
allowed tool action edits the notes file (132929600). The update prompt
preserves predefined section headers, requests a bounded structured account
of task state, files, errors, learnings and results (127020268-127026406).
The fork reads the notes file, not just a model-generated in-memory string.
`iy8` can later use these notes for a cheaper compaction, but only under its
separate gate. Separately, context accounting reads `CLAUDE.md` memory files
(`Nif`, 129885486), and the binary describes saved memory as potentially
stale (125320405). Session notes, project instructions and compaction summaries
must not be conflated. Neither note quality nor price advantage was measured.

**Proposal:** keep immutable user instructions distinct from generated
session notes and workspace memory. Store provenance, source revision and last
verified time for generated notes; verify file-dependent facts before relying
on them. Make note extraction asynchronous when safe, but define ordering at
turn/compaction boundaries. Measure extra note-extraction spend against saved
compaction spend and task quality, including stale-note failures.

## Token accounting and tool-result budgets

**Observed:** `kif`, `Nif`, `Eif` and the context analyzer at 129885486-129891576
estimate system sections, project memory, built-in/MCP and deferred tools,
agents, skills and messages separately. Deferred schemas are counted in an
informational category but excluded from occupied-token sum; reported API
usage can override the estimated category sum for the displayed total.
Message breakdown includes tool-call/result and attachment categories. This
is better attribution than Drift's current UI heuristic, not proof of exact
provider tokenizer equivalence.

**Observed:** per-tool result processing at 125237908-125239000 defaults to
400,000 characters when no limit is supplied, otherwise caps finite tool
limits to 50,000 characters unless configured per tool. Empty outputs receive
a short completion marker. Large text-only content is persisted to a
session-scoped file named by tool-use ID and replaced with a 2,000-character
preview plus path and size; a newline near the end is preferred. Non-text
arrays and content containing image blocks are not persisted by this path.
Persistence errors return the original result. The per-message budget path
`qo4` at 125241781 is independently gated by
`tengu_hawthorn_steeple` (125238500), uses a default 200,000-character
window and chooses the largest fresh result first to shed enough content.
It records seen IDs and replacements so normalization/retries reapply
replacement deterministically. These are two different limits, and the
per-message budget must not be described as unconditionally enabled.

**Observed in Drift:** `src/engine/context-breakdown.ts:7-72` estimates text
at four characters per token, tool-call input by number of keys times 16,
and attributes the unexplained remainder to system/tools. It does not model
compact markers, restored tail, real schemas or media. Its total comes from
reported usage. OpenCode already prunes completed tool output after a protected
tail and converts pruned output into a placeholder in the model view
(`session/compaction.ts:273-316`, `session/message-v2.ts:297-300`).

**Proposal:** report two numbers per request: actual provider usage from the
response, and an estimated preflight breakdown of the *prepared request*.
Record cached read, cached write, ordinary input, output and optional
provider-specific usage. Account separately for deferred catalog metadata,
loaded schemas, system blocks, media, tool calls, result previews, full
retained outputs and summarized history. Persist oversized text with a
session-scoped retrieval handle and deterministic replay; guard against stale
files and missing attachments. Keep provider token estimators pluggable.

## Portability, tradeoffs and validation

| Mechanism | Portable core | Provider-specific edge | Main cost or uncertainty |
| --- | --- | --- | --- |
| Local deferred registry and search | Yes: ranking, discovery state, permissions, stable names | Anthropic `tool_reference`, `defer_loading`, beta/gateway support | Extra model search turn and dynamic schema support elsewhere |
| Stable prompt structure | Yes: deterministic sections and revisions | Cache scope, breakpoint placement, TTL, cache-edit semantics | Cache invalidation depends on provider serialization |
| Budgeted compaction and notes | Yes: transcript projection, note revision, recovery | Exact tokenizer and provider context limits | Summary calls, lost detail, stale notes |
| Read evidence and output persistence | Yes: filesystem state and retrievable result handles | Media encoding and tool-result protocol | IO overhead and file lifecycle |
| Context display | Yes: prepared-input categories and measured usage | Provider-specific accounting fields | Estimates cannot be equated with billed tokens |

Concrete experiments required before a core replacement claims a speed win:

1. Replay matched many-tool workloads with fixed model, permissions and
   catalog. Compare full tools, Jev server routing and deferred per-tool search
   on correctness, failed schema calls, model turns, schema bytes, cache
   read/write/uncached tokens, TTFT and total latency. Include empty/pending MCP
   servers, tool catalog changes and compaction between discovery and invocation.
2. Exercise provider adapters for `tool_reference`, dynamic tool definitions,
   schema validation, cache marker behavior and unsupported gateways. Test a
   provider with no deferred-schema feature and verify graceful fallback.
3. Resume long sessions after compaction, crash and file changes. Check that
   discovered tools, plan/task state, file evidence, interrupted calls and
   summaries agree with actual repository state. Include too-long retries,
   circuit-breaker behavior, time-gap feature on/off and missing notes.
4. Feed identical model-prepared requests into the category estimator. Compare
   estimates to provider usage, including multimodal payloads, cached tokens,
   persistently truncated text, and both ordinary and notes-based compaction.
5. Run repeated task-quality trials with cold and warm caches. Track median
   and p95 end-to-end latency plus total provider calls and cost. None of these
   benefits is established by static binary inspection.

No core or upstream source was changed for this research.
