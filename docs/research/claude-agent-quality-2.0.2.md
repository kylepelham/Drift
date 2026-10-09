# Making Drift a better coding agent

Investigated 2026-10-06 against `next/2.0.2`, `4eb75f8023`. This follows
[the compatibility gap pass](claude-gaps-2.0.2.md), but changes the question:
which mechanisms should improve correct completed code, useful responses and
time to completion, rather than make Drift resemble another product?

The native engine already removed most local orchestration overhead. Its
recorded performance comparison is in `docs/engine-rewrite.md`. The next gains
should come from fewer unnecessary model requests, less irrelevant context,
better evidence and fewer repair cycles. A faster wrong patch is a loss.

## What this pass established

The same Claude Code 2.1.85 artifact was checked as bytes, not executed.
Binary SHA-256:
`4a336dc188dc44289c801a33ba1868fa2b2b39d345e9a3316327218048c91669`.
Readable source SHA-256:
`a96b01820b7d0036d1d0bf31563cf5d368f586f62f7d655339fbcdd9cadf43d9`.
Both hashes matched again. Source offsets below apply to that artifact only.
No account flags, credentials or paid inference were inspected.

An isolated real `drift-engined` ran through the existing HTTP conformance
harness. Its home and database were temporary, with loopback fake Anthropic
and MCP servers. Fixture commands were harmless. The running desktop app,
its database and project files were not used. The harness captured outbound
requests, stored outcomes and the resulting fixture files.

These are observed data-loss and request-content results, not a benchmark
of live models' coding ability. Scripted replies deliberately exercise the
client's boundaries; they do not measure how often a real model makes the
same decisions. Bytes and characters below are not provider token counts.

| Probe | Observed result |
| --- | --- |
| Read a file, change a different field outside Drift, then submit a stale whole-file Write | The outside edit was overwritten. The call reported success. |
| Compact after an earlier constraint, invoked skill and file read have left the retained tail | A scripted summary omitting them left none of the three in the next request. There was no independent restoration of the current file either. |
| Approved post-write check passes, is missing, or exits with a failure | Pass and missing checker produced the same model-facing write-result text apart from the filename. A failing check's output did reach the model. The fake model could still finish with "Done". |
| Offer 5, 40 or 200 unused synthetic MCP tools | MCP schemas alone occupied 7,963, 63,468 and 317,328 JSON bytes. No MCP tool was called. |
| Read six individually small files, then send unrelated follow-ups | 86,291 characters of tool results appeared in each of three requests, 258,873 characters resent in total. |

## 1. Make the model's file evidence current before it writes

This is the first fix I would make. It affects correctness directly and does
not require another inference call.

Drift's read record stores canonical path membership, retained across restart
(`crates/drift-engine/src/tool/context.rs:10`). Write checks membership, rereads
the current file for format/history, then replaces it with the model's complete
content (`crates/drift-engine/src/tool/write.rs:54`). It never compares the
current bytes with the bytes the model actually saw. Per-path locks serialize
Drift's writers, but do not validate an old model view against an outside edit.

The fixture was exact:

1. Read `public=alpha\nuser_owned=original\n` through the real Read tool.
2. Outside the engine, change only `user_owned` to `EXTERNAL_EDIT` and restore
   the original modification time.
3. The fake model sends a whole-file Write changing `public` to `beta` but
   retaining its old `user_owned=original` line.
4. The file becomes `public=beta\nuser_owned=original\n`. The external edit is
   lost; the tool result says `Wrote stale.txt (+2 -2).`

The existing atomic writer prevents partial replacement. Undo can retain
the pre-write version, but neither fact makes this overwrite correct.
This is not a simultaneous filesystem race: the outside edit finished before
Write began. It also defeats a freshness fix based only on increasing mtimes.

Claude's inspected Edit/Write paths compare read state and timestamps, but
their equal/older-mtime and direct-write-fallback weaknesses remain documented
in `claude-tools-prompts.md`. Copying them would not fix this fixture reliably.

Adoption target: retain byte identity and read coverage with the read record;
check current identity while holding Drift's existing mutation locks before
an overwrite. A stale view must produce a bounded, useful conflict response,
not silently widen permission or overwrite the file. Partial reads and shell
reads cannot pretend they established a complete whole-file view. Update
evidence after an engine write and formatter/check rewrite, and preserve its
meaning through speculative reads, forks and restart. An outside process can
still race after a check; do not promise cross-process compare-and-swap.

Acceptance: preserve the outside edit for normal, equal-mtime and older-mtime
changes; no regression to BOM/CRLF, partial reads, early reads, undo, path
approval or concurrent-session tests. Measure refusal/re-read rates so this
does not become a blanket extra read before every valid edit.

## 2. Keep a task brief and evidence through compaction

The largest long-task quality opportunity is to stop treating the summary
as the only copy of a task's constraints and working knowledge.

Drift already preserves the active split-turn prompt, two recent turns within
a budget, primary-agent reminders and durable worker delivery. Its summary
prompt requests goals, preferences, decisions and verified versus assumed
state (`crates/drift-engine/src/config/prompts/compaction.txt`). This is useful
guidance, but no separate mechanism guarantees an earlier user correction or
invoked skill remains in the next prepared request.

The probe loaded a skill with an exact API rule and read a file. After three
later turns, manual compaction received a deliberately incomplete summary.
The following request lacked the initial user constraint, the skill body and
both old and new file-version markers. All original history remained stored.
The failure is availability to the next model request, not deleted history.

Claude separates several mechanisms:

- `H0K` asks the summarizer to preserve explicit
  requests, feedback, technical decisions, errors, pending tasks and current
  work. More detailed instructions alone still cannot guarantee fidelity.
- `svH` and its restorers carry invoked skill content, recent file evidence,
  plan references and task state outside the free-form summary. Limits and
  permission behavior are in `claude-gaps-2.0.2.md`.
- Session-note extraction has a separate gate and cadence. `CV1`
  uses defaults of 10,000 tokens before initialization,
  5,000 more between updates and three tool calls. `pV1`
  allows only Edit on the one notes file.
- Notes-based compaction `iy8` falls back when notes,
  anchors or budgets are unusable. `ny8` requires explicit enablement or both
  relevant remote flags. These are not universal default extra calls.
- Cross-session memory is another mechanism. `TzK` and `gC6`
  load a bounded `MEMORY.md` index, with 200-line and 25,000-character limits,
  and point to topic files rather than loading every note.

For Drift, start with cheap state it already knows: the active user task and
later amendments with source-message identities, invoked skill versions,
pending worker IDs/statuses, todo state, changed file identities and recent
verification outcomes. Preserve user text as user text, not a new system
instruction. A generated note must name its source and cannot override a later
user correction. Define when a new goal retires the old brief; blindly pinning
all historical requests would preserve contradictions forever.

Restore only what is needed under a model-specific budget. Skill rules can
be restored from the invoked version; volatile file facts need fresh authorized
reads or explicit stale references. Do not restore large bodies already in
the tail. Put changing task state after the stable cached prefix.

Acceptance: after multiple compactions, the agent still obeys a late user
correction, avoids a previously rejected approach, knows which workers are
already running and resumes with valid file evidence. Test a changed/deleted
file, revoked permission, replaced skill, stopped worker and a deliberately
bad summary. Count extra requests and compaction frequency. Periodic model
note-taking and cross-session memory come later, only if they earn their cost.

## 3. Make verification evidence govern completion claims

The goal is fewer incorrect "done" responses, not longer final answers.
Use actual tool/check evidence before adding a second reviewer model.

Drift already runs trusted checks once per writing step, four at a time within
one budget, and deduplicates repeated problem output. That is a useful base.
But `checks_note` ignores `Unavailable` and sends no pass receipt
(`crates/drift-engine/src/session/turn.rs:2093`). Metadata retains the distinction;
the normal provider projection does not send that metadata as check state
(`crates/drift-engine/src/session/convert.rs:175`).

The missing-check fixture's metadata said `unavailable`, while the next model
request only said `Created unavailable.txt (1 line).` A passing check likewise
only said the file was created. A known failing check supplied its error text,
but the scripted "Done" response still ended the turn. This proves there is
no completion guard for those receipts, not that a real model always ignores
failures. A check failure must not retroactively erase a successfully recorded
file mutation or be confused with a failed Write.

The check runner reads both output pipes fully into memory, then shows the
first 4 KiB of a failure (`crates/drift-engine/src/edit/check.rs:134`). It does
not preserve a full-result retrieval handle. A long build log can bury the
actionable failure beyond that prefix, increasing another search/run cycle.
This is a source-verified bound/retrieval issue, not a memory benchmark.

Claude's normal `y4_` runs configured Stop or
SubagentStop feedback, and `dcf` can feed blocking feedback into another turn.
That is not an unconditional code-correctness judge. The auto-mode `PS8`
handoff classifier checks block rules and dangerous
work, not whether the feature meets its spec.

The readable bundle also contains a long verifier prompt assigned to `hfz`.
This pass found the identifier at its declaration and assignment,
not an ordinary agent getter using it. `Ph8` lists general, statusline setup,
Explore, Plan and guide agents in this artifact. Do not infer an always-on hidden verifier from
that prompt or from the `verification_agent` query-source label.

Adoption target: store and project compact verification receipts with command,
outcome, scope, code revision and retrieval handle. Distinguish passed, failed,
unavailable, denied, timed out, not run and stale. A later edit invalidates the
relevant receipt. This is stronger than appending another paragraph asking the
model to be careful.

Before completion, give one bounded corrective opportunity for known required
failures or unfinished acceptance items. If it cannot resolve them, end honestly
as blocked/incomplete with the actual reason. Do not force an endless test loop,
run unapproved commands, treat unrelated baseline failures as new regressions,
or prevent a user Stop. Choose narrow checks while editing and required broader
checks at a stable completion boundary; rerunning a large suite after every
small edit is not necessarily faster or better.

Response policy should use those receipts: what changed, what passed, what was
not checked, and remaining risks. An unavailable checker is not a pass; an old
pass before later edits is not proof of the final tree. A reviewer for complex
changes should inspect the actual diff and run a task-specific adversarial
probe, not confirm the implementer's summary. Make that extra inference opt-in
or stakes-based and respect the user's delegation rules.

Acceptance: a failed required check cannot end as verified success; a missing
checker is explicit; a pass is tied to the code it checked; the final decisive
error in a noisy log remains retrievable. Track false success, regressions,
unnecessary check runs and time to correct completion.

## 4. Stop paying for tool schemas the task does not use

This is the strongest measured speed candidate for MCP-heavy sessions.
The probe captured real Anthropic request bodies with no MCP invocation:

| Connected synthetic tools | MCP schema bytes | Whole request bytes |
| ---: | ---: | ---: |
| 5 | 7,963 | 26,968 |
| 40 | 63,468 | 82,473 |
| 200 | 317,328 | 336,333 |

Each fixture tool had twelve string fields and a moderately long description.
These sizes are not a claim about every real catalog. They establish linear
request growth even for a simple reply that uses none of the tools.

Claude defers definitions and carries discovered names across compaction.
The model/endpoint gates and portable-provider limits remain important.
Drift currently clones every permitted schema in `Offer::specs`
(`crates/drift-engine/src/session/turn.rs:1913`).

Adoption target: keep built-ins and small catalogs direct; for large catalogs,
offer cheap local exact/prefix/keyword discovery with a compact catalog index
and selected definitions. Pin the same tool implementations and permissions
as today. Never select tools through an extra routing model. Keep discovery
through compaction and restart; providers without native references receive
explicit schema updates on the next request. Benchmark against the existing
cached full-catalog path, not an artificially uncached baseline.

A schema search costs a model round trip and changing definitions can cost a
cache miss. Therefore a smaller request is not itself a speed win. Compare
correct tool selection, missed tools, model calls, cache usage and time to task
completion on 5/40/200-tool tasks. Preserve full-catalog fallback when selective
discovery is unavailable or costs more than it saves.

## 5. Size retrieved evidence for the question, not the tool's maximum

Drift's per-result spooling already prevents one huge result from filling the
request. It does not prevent numerous smaller results from doing so.
The six-read fixture produced 86,291 tool-result characters in each subsequent
request. All results were below the individual 64 KiB limit. Fake usage stayed
small deliberately, so this proves the lack of an aggregate preparation budget,
not a failure of real usage-triggered compaction.

Claude's gated `qo4` persists selected fresh results,
records replacements by call ID and reapplies them. `er4` selects large results
to shed. It is a per-message budget, not a whole-conversation semantic ranking
system; do not mistake it for one. Time-gap microcompaction defaults off in
this artifact. Rewriting old results can lose cache reuse.

The first improvement need not be an embedding index or another agent.
Directed search should return the relevant file, surrounding code, reference
locations and an honest continuation path. Current Grep has only pattern/path/
include and emits matching lines (`crates/drift-engine/src/tool/grep.rs:25`).
For callers needing nearby implementation context or filenames only, a small
optional context/output-mode contract could remove a follow-up request. Keep
authorization and truncation visible; a search hit is not a whole-file read.

Next, add a prepared-request budget that protects the active task, current
errors, newest useful steps and tool-call/result pairing. Replace older bulky
evidence with typed summaries or retrievable references, never destroy the
stored transcript. Do not drop the critical error line, a relevant public API,
or a user constraint merely because it is expensive. Prefer deterministic
projections; adding a summarizer call for every result defeats the point.

Acceptance: the model finds the same relevant code with fewer serial requests,
continues after truncation without rerunning work, and still catches a regression
whose evidence is near the end of a large output. Measure cache misses and
retrieval calls as well as request size. Prevent immediate recompaction loops.

## 6. Delegate selectively and return evidence, not just confident prose

Drift already supports useful parallel workers. More workers are not a general
quality improvement. They add prompts, results, file contention and duplicated
exploration unless the jobs are genuinely independent.

Claude's `g7f` recommends direct tools for directed searches and
calls broad exploration slower. Its threshold literal is three queries, a
heuristic, not a measured optimum. Its Explore definition pins
Haiku and omits CLAUDE.md. Drift's Explore is read-only but inherits the parent's
model unless pinned through agent config/Settings
(`crates/drift-engine/src/tool/task.rs:77`). Drift should retain necessary
repository rules; dropping them for speed can produce worse code.

Use direct parallel local tools for obvious lookups. Use background workers
for independent investigations or disjoint changes where their work overlaps
usefully with the main job. A cheap exploration model is worth testing on
retrieval tasks, not silently substituting for security review or difficult
implementation. A resume of the existing worker often beats launching another
one to rediscover the same files. Respect explicit no-delegation rules.

Worker handoffs should name files/ranges, changes, tests actually run, failures
and unknowns, with durable result references. Current results are clipped to
the first 20,000 characters (`crates/drift-engine/src/session/tasks.rs:23`,
`crates/drift-engine/src/session/tasks.rs:543`). The full child transcript
survives, but the ordinary handoff provides no full-reply retrieval file.
If the final warning or evidence is after that cutoff, asking the worker again
costs another model request. Prefer a compact evidence-bearing receipt and a
parent-scoped full-result handle; do not automatically summarize every worker
with a second model.

Acceptance: independent jobs shorten completion without duplicating reads or
overlapping writes; failed/incomplete workers cannot masquerade as verified
results; important late warnings remain retrievable. Report total child-plus-
parent inference and repair time, not just the main conversation's speed.

## What I would not prioritize

- The context meter is useful observability, but its attribution fix alone
  does not change the model's evidence or improve its code.
- A universal reviewer model, more thinking on every task or a much larger
  system prompt adds cost without an established quality benefit.
- Agent teams, deeper delegation and an autonomous workflow engine are not
  needed for any of the fixes above.
- Fuzzy editing is not the answer to stale evidence. Keep exact matching;
  measure miss/re-read rates and improve the supplied evidence first.
- Automatic continuation after output exhaustion may improve response
  completeness. Claude bounds its recovery at three attempts. Evaluate this
  after evidence and verification work, with Stop/budget fencing and no replay
  of cut-off calls, not as an unlimited retry policy.
- Automatic project memory can reduce repeated discovery, but stale beliefs,
  privacy and unwanted instruction persistence are real costs. Start with
  session-scoped, source-linked task state before cross-session extraction.

## Implementation and measurement order

1. Fix stale whole-file overwrites and add the content-identity regression.
2. Project explicit verification outcomes, preserve noisy check output and
   tie receipts to the checked revision. This directly improves final answers.
3. Preserve active task state and invoked skill evidence through compaction;
   restore selected fresh files within budget. Add a bounded completion guard
   only over explicit requirements and real receipts.
4. Measure selective tool discovery and request-result budgets independently.
5. Trial targeted retrieval and cheaper, selective exploration on the same tasks.

Use the existing fake-provider harness for deterministic acceptance and failure
cases. It can prove correct request contents, no duplicate effects, cancellation
and recovery. It cannot prove the model chooses better code.

For a first live screening trial, freeze a small task set covering a localized
bug, cross-file API change, late user correction after compaction, external
file change, missing/failing checker, large MCP catalog and noisy build output.
Run baseline and one candidate on identical starting trees with the same model,
effort and explicit budgets. Include short tasks as controls. Use hidden tests
and a diff check against forbidden changes. Repeat stochastic tasks and report
every failure. A small pilot can reject a bad idea; it cannot justify a public
percentage quality claim. The larger paired/held-out method remains in
`engine-evaluation.md`.

Primary outcomes: accepted correct code without forbidden changes, truthful
completion status and time to that outcome. Record provider requests, input/
cache/output usage, human repair, extra re-reads/checks, child work and retries.
Measure response quality by correctness, evidence and whether it answers the
user's actual question, not by length or polished wording. No live improvement
percentage is claimed by this investigation.

## Reproduction

Private probe:
`docs/research/private/probes/claude-agent-quality-probe.ts`.
It imports `tests/conformance/harness.ts`, isolates the engine's home/data,
asserts the observed outcomes above and cleans up its headless process, local
services and temporary workspace. It never starts a visible chat thread or
reads the desktop app's database.

The probe reproduced the outcomes above, including the stale overwrite with
a checked millisecond modification-time match. Raw proprietary source and indexes remain outside the repository.
Product behavior is unchanged. The recommendations are not implemented fixes.

The probes and the commands that run them are kept locally in `docs/research/private/`, which is
not committed.
