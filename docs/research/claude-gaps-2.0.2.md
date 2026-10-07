# Claude Code gaps after Drift 2.0.2

Investigated 2026-10-06 against `next/2.0.2` at `4eb75f8023`. This is a new
comparison against the native engine, not the old OpenCode runtime. No product
behavior changed in this pass.

The [deeper agent-quality pass](claude-agent-quality-2.0.2.md) reprioritizes this
list around material outcomes. It reproduces a stale whole-file overwrite and
missing verification feedback, and measures unused request contents. Existing
staged-write safety below must not be read as external-edit freshness checking.

## Evidence

The handoff's `re/claude-2.1.85.exe` is still the inspected artifact. Its size is
237,718,176 bytes and its SHA-256 is
`4a336dc188dc44289c801a33ba1868fa2b2b39d345e9a3316327218048c91669`.
The readable source range has SHA-256
`a96b01820b7d0036d1d0bf31563cf5d368f586f62f7d655339fbcdd9cadf43d9`.
Both hashes were rechecked before inspecting bounded source windows and callers
from the existing structural index. Offsets below are end-exclusive binary byte
ranges. Minified names apply only to this artifact.

Claude was not executed. No credentials, remote flags, paid calls or account
entitlements were inspected. This does not establish behavior in newer Claude
releases or show that any missing feature improves task quality or latency.

Two harmless fixtures ran against Drift's real config loader and exported UI
estimator. Other differences below are source-verified, not reproduced failures
of a model doing a task.

## Findings, in recommended order

### 1. Imported skill execution metadata is silently ignored

This is the most important compatibility gap. Drift searches `.claude/skills`
and loads the instructions, but that does not make its skill contract equivalent
to Claude's.

Claude's `Fo6` parses `disable-model-invocation`,
`user-invocable`, `context`, `agent`, `model`, `effort`, `allowed-tools`, hooks
and shell settings. The Skill validator explicitly
refuses a skill marked `disable-model-invocation`. The fork paths traced in
`claude-async-workers.md` run a child context; `context: fork` does not itself
mean background execution. `zhH` activates dormant
path-scoped skills when touched paths match. Its callers include Read, Edit
and Write.

Drift's `config::Skill` holds name, description, directory, instructions and
argument hint only (`crates/drift-engine/src/config/mod.rs:377`). `add_skills`
does not interpret the other fields (`crates/drift-engine/src/config/mod.rs:705`). `prompt::system` lists each
permitted loaded skill (`crates/drift-engine/src/session/prompt.rs:77`), and
`tool::Skill::run` returns its pinned body inline
(`crates/drift-engine/src/tool/skill.rs:30`).

The fixture created a harmless `.claude/skills/user-only/SKILL.md` carrying
`disable-model-invocation: true`, `user-invocable: false`, `context: fork`,
`agent: explore`, `allowed-tools: Read`, and `paths: src/**/*.rs`. Drift still
exposed its description in the model-facing skill list and its slash command
in `Config::commands`. No skill or shell command ran.

Existing Drift tool permissions still apply. This does not bypass them or
prove arbitrary code execution. It does mean a user-only workflow can be
advertised as model-callable, and a workflow intended for an isolated child
can instead load into the main conversation. `allowed-tools` in Claude is
permission-related metadata, not evidence of a universal read-only sandbox.

Recommended first fix: honor the two invocation flags and refuse direct model
invocation of a user-only skill. Define user-command authorization separately
because Drift currently routes skill slash commands through the same tool.
For execution-changing fields not yet supported, report the incompatibility
instead of silently flattening it. Do not add a JavaScript plugin host or
turn `context: fork` into a visible conversation.

### 2. The context breakdown does not use the engine's compaction view

Claude's context analyzer `tF8` counts system text,
tools, memory, skills and messages separately. Deferred schemas have an
informational category excluded from occupied tokens. Reported API usage can
replace its estimated total. Those estimates are not exact provider tokenization.

Drift's `src/engine/context-breakdown.ts:29` starts counting at the latest
assistant summary, regardless of whether it succeeded. The engine instead
uses the latest finished, nonempty summary plus its retained pre-summary tail
(`crates/drift-engine/src/session/compaction.rs:55`). Native adaptation preserves
`summary: true` even on failed summary messages
(`src/engine/native/adapt.ts:78`, `src/engine/native/adapt.ts:92`).

The fixture supplied a 4,000-character completed tool result before a
400-character summary and an API total of 1,200 tokens. The breakdown reported
zero tool tokens, 100 assistant tokens and 1,100 system tokens. Without the
summary, the same result counted as 1,004 estimated tool tokens. A failed
summary also hid the earlier result. The engine's existing long-turn compaction
fixture proves pre-summary tool results can remain in the request.

This affects attribution, not the reported total or the engine's compaction
threshold. A minimal fix needs the successful boundary and its tail cutoff;
the adapter drops that cutoff (`src/engine/native/adapt.ts:111`). Longer
term, calculate categories from the prepared request in the engine, including
actual tool schemas, media and omissions, rather than assigning every
unexplained token to system text.

### 3. Compaction does not explicitly restore active file and skill context

Claude's full compaction `svH` saves read state,
clears it, then assembles restoration attachments after producing the summary.
This pass rechecked the individual restorers rather than assuming the summary
contains them:

- `u0K` chooses recent files, excluding plan and
  memory files handled elsewhere. The constants nearby limit selection to
  five files, 5,000 estimated tokens per read and 50,000 aggregate tokens.
- `Tp6` checks permission/validation and invokes
  the normal Read implementation. If the compact-mode read exceeds its
  limit, it can return a file reference instead of content. Therefore the
  restore is not a guarantee of five complete files or unlimited implicit reads.
- `x0K` restores invoked skill bodies and paths,
  newest invocation first, with per-skill and total budgets. The registry
  `kjH`/`Q_8` records content when invoked, scoped by agent.
  This restores the invoked version, not necessarily today's SKILL.md.
- `uy8`, `m0K` and `p0K` restore a plan reference,
  plan-mode reminder and worker status. Discovered tool names go on the
  compact boundary through `tU`.

Drift already keeps the user's prompt, recent tool steps, primary-agent
reminders, durable worker results and a bounded tail. Successful compaction
clears the subdirectory-instruction display set, so another read can show
those instructions again (`crates/drift-engine/src/session/compaction.rs:154`,
`crates/drift-engine/src/tool/mod.rs:61`). It does not
automatically reread files or reattach invoked skill bodies outside the tail.
The persistent read record stores path identities, not file contents or
recency (`crates/drift-engine/src/tool/mod.rs:42`).

The gap appears when the important read or skill invocation falls before the
tail: the next request depends on what the summary retained or another read.
This is a missing recovery feature, not proof that every compacted task fails.
First add a fixture that loads a skill, reads a file, pushes both out of the
tail, compacts, changes the file externally and captures the next request.
Any restoration should obey current permissions, carry version/provenance,
avoid duplicate tail content and fit the active model's input budget. The
plan-file exception already remains open in M5; keep it narrowly scoped so
the plan agent cannot write ordinary project files.

### 4. Large tool catalogs still send every permitted schema

Claude's `vrH` selects deferred search only after
model/tool/mode checks. `Ph` can reject an unconfigured
non-first-party endpoint. The model check `z88` uses a remotely configurable
unsupported-name list, not a general negotiated wire-capability guarantee.
`tU` rebuilds discovery from tool-reference results
and compact-boundary metadata. The request builder then offers the discovered
deferred schemas, as recorded in `claude-context.md`.

Drift correctly filters tools by agent, permissions and delegation boundary,
and holds the actual tool instances for the admitted turn (`crates/drift-engine/src/session/turn.rs:993`).
But `Offer::specs` clones the entire permitted catalog (`crates/drift-engine/src/session/turn.rs:1913`) and
`step_request` sends it every step (`crates/drift-engine/src/session/turn.rs:865`). There is no session-scoped
discovery set or portable search tool.

Do not replace this with the deleted Jev router. Measure full catalogs against
selective schemas on a many-server fixture first. If worthwhile, discovery
must retain the pinned implementation/schema version, survive compaction and
restart, and never expose tools an agent cannot invoke. Providers lacking
reference-block support need explicit schema updates on the next request.
Static inspection does not establish a speed win or gateway support.

### 5. Per-result limits are not an aggregate request budget

Drift already spools large results with retrieval paths, and shell capture is
bounded (`crates/drift-engine/src/tool/spool.rs:8`). The shared result limit is
64 KiB; retained results replay through `crates/drift-engine/src/session/convert.rs:175`. Numerous
individually valid results can still occupy most of a request before the next
usage-triggered compaction. There is no aggregate result-trimming stage in
`step_request`.

Claude has a separate gated per-message result budget, traced in
`claude-context.md:253`. This pass also verified that `eU`
invokes the time-gap `e5f` branch and otherwise returns
the input unchanged. Its fallback configuration
is disabled, with a 60-minute gap and five recent eligible calls kept.
Do not describe microcompaction as always enabled or assume it is free:
changing an old result can invalidate a cached request prefix.

Treat aggregate budgets as an experiment after request accounting, with
pair-preserving replacements, durable retrieval handles and cache measurements.
Saved session notes and project memory remain separate optional work. Nothing
here proves an extra summarization or memory model call pays for itself.

## Already covered, deferred, or deliberately different

| Behavior | Drift 2.0.2 conclusion |
| --- | --- |
| Foreground and background workers, attributed asks, parent completion delivery, Stop, restart interruption, finished-worker follow-ups | Implemented. Do not reopen M3 based on the older RE contract. |
| Move a live foreground worker to the background, or steer a running background worker | Still M5. Rechecked Claude's `Df_` and `_B8`: user promotion updates task state and resolves the existing foreground registration's signal. This is not a new launch or a prose keyword switch. Drift's `resumable` accepts terminal jobs only (`crates/drift-engine/src/tool/task.rs:120`). |
| PreToolUse, PostToolUse, Stop and SubagentStop extension points | Still M5. Formatter, check and LSP runners are not a general hook implementation. A bounded completion-feedback loop remains a design choice; do not assume Claude's Stop-hook loop is universally bounded. |
| Automatic continuation after output exhaustion | Claude's `dcf` allows three recovery attempts (`Fcf=3`). Drift surfaces `Ending::Length` and settles unrun calls (`crates/drift-engine/src/session/turn.rs:1154`). Consider optional, bounded continuation of text only; never execute a cut-off call or duplicate an earlier mutation. Not established as a release bug. |
| Read/write correctness and undo | Drift already has serialized file writers, exact checks, staged replacements, rollback and durable recovery. Claude's fallback direct write and timestamp-based checks are not improvements to copy. |
| Cache markers, warm-cache summaries, lean-summary fallback, overflow recovery and retry limits | Implemented. Missing selective discovery is distinct from missing basic caching or compaction. |
| Model-family prompts, truthful shell dialect, paged reads, full-output links and post-edit diagnostics/checks | Implemented. The old reports describe the pre-cutover state. |
| Live account activation, newer Claude versions, quality and paid performance | Not measured by this pass. |

## Reproduction and checks

Private scratch helpers live under `docs/research/private/probes`:

- `claude-202-source-probe.cjs` checks both hashes, resolves callers through the
  existing index and prints bounded source windows. It never evaluates source.
- `claude-202-config-probe` depends on Drift's library and creates/removes one
  harmless skill fixture. Cargo uses the repository's existing `target/`.
- `claude-202-gap-probe.ts` imports the real context estimator and asserts the
  three attribution results above. It uses synthetic messages, not live history.

The probes confirmed the current gaps; they are not passing regression tests
for fixes. Raw executable source and generated indexes remain outside Git.
The full repository gates validate the documentation-only final tree separately.

The probes and the commands that run them are kept locally in `docs/research/private/`, which is
not committed.
