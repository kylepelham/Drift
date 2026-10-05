# Claude async workers: RE and Drift implementation contract

Investigated 2026-09-30 for M3 at Drift commit
`1998476160d589cd238f1f8036ae0f752f182eb0`. This extends the earlier
[execution trace](claude-execution.md#subagent-boundary), which established the
child runner but did not fully reconstruct asynchronous launch and delivery.
No worker, visible chat thread, login or paid model call was started for this research.

During tracing, `6f7dbedb0` landed the user-reviewed branching and completed-worker
sidebar fixes. The canonical-plan update preserves that implementation and adds
only the pending async-worker contract; it does not mark background execution done.

## Artifact and evidence discipline

Installed executable: `C:\Users\KylePelham\.local\bin\claude.exe`, reporting
Claude Code 2.1.85; 237,718,176 bytes; SHA-256
`4a336dc188dc44289c801a33ba1868fa2b2b39d345e9a3316327218048c91669`.
The first readable JavaScript region is `[120866728,133121000)` with SHA-256
`a96b01820b7d0036d1d0bf31563cf5d368f586f62f7d655339fbcdd9cadf43d9`.
Hashes were rechecked. Offsets below are zero-based bytes in this exact file.

Bounded source windows and the earlier AST index were used to follow definitions
and callers. The helper is outside git in the approved temp directory as
`drift-async-re-probe.cjs`. It reads bytes, hashes the artifact and prints bounded
windows; it never executes extracted code. Bulk proprietary code is not copied
into Drift. See [binary evidence](binary-evidence.md) for the container method.

The local read-only `examples/claude-code` reconstruction declares version 2.1.88.
Its descriptive module/function names helped navigation, but it is neither this
binary nor authoritative upstream source. Binary claims below were checked against
2.1.85. Current official docs describe later versions too; those are a separate
evidence column, not proof that this executable enables every newer feature.

Labels: observed means code and caller visible in the hashed artifact; conditional
means a flag or runtime mode selects the path; proposed means Drift behavior.
Static inspection does not establish live remote flags, paid performance, crash
durability or account-specific activation.

## The three concepts

1. **Foreground worker:** the launch tool awaits delegated work and returns its result.
2. **Background worker:** the tool returns a launch receipt, the main model can
   continue, and a later completion notification supplies the result. Several can
   execute simultaneously.
3. **Independent conversation:** a separate user-facing session with its own goal
   and subsequent messages. It can inherit selected context or an explicit transcript
   fork. It is not a subagent merely because it runs in the background.

Claude also has agent-team and background-main-session paths. These do not make
every background subagent a persistent normal conversation. Drift should adopt
async worker mechanics, not collapse these concepts into one spawn operation.

## Reconstructed lifecycle

```text
Agent call + execution-mode decision
  -> allocate agent/task identity and resolve model, tools, context
  -> register local_agent task with controller and progress state
  -> begin the ordinary child query iterator asynchronously
  -> return async_launched receipt immediately

child query iterator
  -> persist sidechain messages, update progress
  -> completed / failed / killed
  -> finalize result and worktree metadata
  -> claim notified flag
  -> enqueue task-notification for the parent

parent message queue
  -> consume notification in a later model turn
  -> report/use the result, without turning it into another launch-tool response
```

### Launch and mode selection

The Agent schema contains `run_in_background`; definition metadata also has a
background setting. `KaK.call`, `[128889797,128901230)`, contains the async selection
expression after agent/context resolution. In this artifact its fork helper `qb`
is compiled to `return false`, coordinator/assistant terms are false, and its
proactive module is null. The reachable ordinary choice is therefore explicit
request or agent definition, unless background work is disabled. Other versions
can enable those extra terms. The `qr6` schema at about 128888500 hides the argument
when disabled or fork mode qualifies. `nCf`, `[128886676,128886801)`, has an optional
120,000 ms foreground-to-background threshold controlled by env/remote flag;
its fallback is zero. This is not proof auto-backgrounding is universally active.

### Full selection path, including skills and the suspected chat keyword

There are two different decisions: the model chooses a tool request from its
instructions/context, then deterministic client code routes that request. We
can RE the latter and inspect the instructions affecting the former. We cannot
recover the model's internal reasoning or training from the CLI executable.

```text
render Agent schema/description
  -> model emits Agent arguments
  -> validate agent, permissions and calling context
  -> special named-team branch? handle teammate separately
  -> resolve ordinary agent definition or eligible fork
  -> prepare task context and effective execution mode
  -> background disabled? synchronous path
  -> explicit background / definition background / enabled forced mode?
       yes: register task -> start WS8(Tk) detached -> async_launched receipt
       no: register foreground -> await Tk -> completed result
           optional later background signal transfers the worker
```

| Input or branch | Where traced | Result in this executable |
| --- | --- | --- |
| Model-facing guidance | `FLK`, `[127044531,127055497)` | Foreground when the result is needed to proceed; background for independent work; receive a notification, do not poll. This is model guidance, not a keyword parser. |
| Argument and schema gate | `qr6` near 128888500; `mEH` initializer near 128887000 | `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS` hides the argument and forces ordinary work sync. |
| Agent definition | `l1_`, `[129879767,129882301)` | Markdown `background: true` becomes the background setting; false/omitted does not force sync over a true tool argument. |
| Named team/context guard | `KaK.call` from 128889797; `BW`, `[123623586,123623631)` | Named teammate launch is a separate early branch; in-process teammate context rejects background subagent requests/definitions. |
| Ordinary mode expression | Around 128893400 in `KaK.call` | Background when argument or definition is true, subject to disable and context guards. An explicit false does not override a background=true definition here. |
| Forced fork mode | `qb`, `[125402194,125402217)` | Compiled off in 2.1.85: returns false. Do not claim this account's default is forced background based on newer docs. |
| Foreground promotion | `nCf`, `_aK`, `Df_` and the race near 128896582 | Optional runtime/user signal, separate from initial selection. The 120s timer is gated and defaults off. |
| Resume worker | `WH8`, `[129483544,129486193)` | Separate entrypoint explicitly runs async using saved transcript/metadata. |
| Skill metadata `context: fork` | `Fo6`, `[129044334,129045149)`; prompt dispatch near 128787386 and Skill call near 128914632 | This keyword belongs to parsed skill configuration, not arbitrary prose. It selects a child execution context. |
| Forked slash command | `xIf`, `[128781291,128782704)` | Calls `Tk` with `isAsync:false` and awaits it in this artifact. `context: fork` does not mean async. |
| Forked Skill tool | `quf`, `[128909613,128911037)` | Also calls `Tk` with `isAsync:false` and awaits the result. |
| Text recursion marker | `w3K`, `[125402217,125402395)`; `M3K` literal near 125404853 | Searches user text for `You are a forked worker process` to reject recursive forks when the fork branch applies. It does not choose async versus sync. |
| Workflow task category | `local_workflow` in task prefix/schema/presentation at 124453136, 127566757 and 128735629 | A different task type/display category. The ordinary Agent selector has no workflow-name/prose keyword predicate. Full workflow-engine activation was not established. |

The readable 2.1.88 reconstruction has a forked-slash-command assistant/scheduled
mode that starts detached work and reenqueues a tagged result. That branch is not
in the inspected `xIf` function and cannot be claimed for 2.1.85. In newer builds
the mode is structured runtime state, not the presence of the word workflow in chat.

**Conclusion:** the relevant chat-facing keyword is likely `fork`, but it is
overloaded. Skill `context: fork`, a context-inheriting worker, an independent
conversation fork and foreground/background execution are separate axes. No
arbitrary-prose keyword switch was found in the traced ordinary Agent and skill
paths. Global workflow engine behavior outside those paths remains unproven.

For Drift, define a single typed mode resolver with recorded reason. Explicit
`run_in_background` wins over an agent default; absent argument uses the configured
default or foreground. A disabled async feature rejects an explicit background
request with a clear error instead of silently making a long job synchronous.
Context inheritance and user-session branching do not force execution mode.
Do not infer mode from words, titles, XML-like user text or model ID substrings.
Skills/commands that execute workers must call this same resolver with explicit
metadata. Inline skill loading remains a context insertion, not an implicit job.

The async branch registers through `NB8` at 128895140, invokes `WS8` without awaiting
the worker, then returns `async_launched`, agent ID, description, prompt and output
file at 128895875. The result renderer at `[128901684,128903981)` tells the parent
that launch succeeded, not that the delegated job finished. It instructs the model
to do non-overlapping work and await the notification rather than repeatedly poll.

### Registration, progress and outputs

`NB8`, `[129737487,129737987)`, registers a running `local_agent` with agent ID,
description, prompt, selected agent, controller, progress counters, background flag,
pending-message list and view-retention state. It also installs cleanup. `R9`,
`[124450526,124450578)`, resolves the task output handle as `<id>.output`.

`WS8`, `[127107298,127108898)`, consumes the child stream, updates tool/token progress
and optional retained-view messages, assembles the final reply, and issues terminal
notifications. The launch result's output file is a retrieval handle, not proof of
a completed answer. The older output tool supports nonblocking status and bounded
waiting, while the inspected model-facing guidance prefers completion notification
and direct retrieval over busy polling. Progress and full transcript are separate
from the compact final result.

### Context and permission boundary

`Tk`, `[128791059,128796094)`, is the ordinary child runner. A non-fork starts from
its supplied task prompt plus its own system/config context. A fork can use parent
history and cloned read evidence. The same underlying `ab` loop executes either.
It writes sidechain transcript/metadata and cleans hooks, read state, agent-owned
MCP clients and shell tasks in its finalizer.

In this executable, `isAsync` normally selects a noninteractive child and
`shouldAvoidPermissionPrompts`; explicit overrides exist. Current official docs say
background prompts are surfaced to the parent in newer releases, with the earlier
auto-deny behavior changing at v2.1.186. Drift should use its existing approval
system with attributed worker requests, not copy the 2.1.85 limitation or silently
grant broader authority. The parent/model cannot approve another worker's call.

### Completion delivery and deduplication

`XS8`, `[129737021,129737263)`, records completed state/result only while running.
`JS8`, `[129737263,129737487)`, similarly records failure. `Q$H`,
`[129735121,129735942)`, claims a task's `notified` flag through an app-state update
before enqueueing. It emits task ID, originating tool-use ID, output file, status,
summary and optional compact result/usage/worktree fields.

`SM`, `[125075877,125075999)`, puts the envelope in the pending message queue with
default priority `later`. This is a later-input delivery path, not mutation of a
finished launch receipt or a second tool result for the same call. Exact timing
depends on the interactive consumer; the inspected producers alone do not establish
all scheduling behavior across every entrypoint.

The notified flag and queue are in-memory state. Do not infer crash-safe delivery
from their in-process deduplication. Sidechain transcript existence likewise does
not prove an interrupted worker automatically resumes execution after restart.

### Foreground-to-background transition

`_aK`, `[129737987,129738809)`, registers foreground work and a background signal.
`Df_`, `[129738809,129739041)`, flips its background flag and resolves that signal.
The Agent call races child-iterator progress with this signal around 128896582.
When backgrounded, it preserves collected context/task identity, closes the old
iterator with a bounded wait, and continues through an async runner before returning
the same kind of launch receipt. This is a meaningful extra state machine, not
merely setting a UI spinner flag.

Drift's first implementation should support explicit foreground/background launch.
Automatic or user-promoted conversion of an already-running foreground task can
follow only if needed; it is not required to obtain genuine async workers.

### Cancellation and sidebar lifetime

`NB8` derives a child controller if one is supplied, otherwise creates a separate
controller. The ordinary async launch supplies the newly registered controller,
not the foreground parent's live iterator controller. `n$H`,
`[129735942,129736202)`, aborts a running task, runs cleanup and marks it killed.
Shutdown cleanup is registered, and the child runner cleans agent-owned shells.
Canceling work is not rollback of file effects.

`iw`, `[127536558,127536693)`, includes only pending/running background tasks in
the active indicator. Terminal jobs are not permanently active. `ii`,
`[126937862,126938054)`, can remove a terminal notified task unless retained for
inspection. A killed-job path uses a 3,000 ms grace interval. These are task-view
lifetimes, not automatic deletion of the persisted conversation/sidechain history.

### Resume and other async session paths

`WH8`, `[129483544,129486193)`, loads a saved agent transcript/metadata, resolves
model/tools/worktree context, registers the same agent identity and starts `WS8`
in background. This shows resumable worker context; it does not establish general
automatic post-crash replay or exactly-once execution.

`FFf`, `[129486452,129487122)`, separately registers `agentType:"main-session"`.
`S__`, `[129488102,129489214)`, starts a background main-session query with copied
messages/query parameters and its own progress/notification path. The visible
`h__` finalizer is `[129487122,129487557)`. Treat this as a separate background
conversation facility, not the standard subagent contract or proof of all public
branch-command semantics in this build.

## Corroborating current documentation

Read through Context7 on the investigation date:

- <https://code.claude.com/docs/en/sub-agents>: foreground/background execution,
  completion notification, isolated context and version-specific permission behavior.
- <https://code.claude.com/docs/en/agent-sdk/typescript>: `async_launched` launch
  variant and `task_notification` completed/failed/stopped lifecycle events.
- <https://code.claude.com/docs/en/agent-sdk/subagents>: ordinary prompt inheritance
  versus context-inheriting worker forks.
- <https://code.claude.com/docs/en/sessions>: independent session resume/branching.

Newer defaults, messaging and UI commands are not asserted for this older binary.
Neither source inspection nor documentation measures better task quality or speed.

## Drift contract to implement in M3

### Keep task execution separate from conversations

- Extend `task` with optional `run_in_background`. Absent argument resolves the
  agent's configured default, or foreground. An explicit value overrides that
  default. Foreground waits for the compact result; background returns a stable
  launch receipt immediately. Record the effective mode and selection reason.
- A worker is a hidden execution record with a child transcript, never a normal
  persistent sidebar conversation. Show active/awaiting-attention worker state in
  parent task cards; completed results stay inspectable there.
- Do not offer `spawn_thread` to the model. `/spawn` is explicit user-owned branching
  through a tool-free handoff draft and approval path. The existing parent-scoped
  `read_thread` can remain for user-requested inspection of a branch; it is not
  worker launch or a polling requirement.
- Background subagents may run concurrently with the main model and each other;
  they do not need independent visible threads to do so.

### Persist the worker and its result delivery

Use the existing child-session transcript storage plus a `subagent_job` identity:
parent session/run generation, originating tool call, child session, job run,
mode, selected agent/model/config/tool revision, status, usage and result reference.
Secret credentials do not go in job JSON. Repeated launch delivery resolves to the
same job through the originating admitted call identity.

States: `queued -> running -> waiting_permission|waiting_question -> running ->
completed|failed|cancelled|interrupted`. Completion and a unique parent-notification
record commit together. Delivery is separate: `pending -> attached -> consumed`.
Attach once to the parent's durable context at a safe provider boundary, with
engine-origin provenance. Group ready completions rather than starting a model
request for every individual delta/job.

The launch tool call stays resolved as launched. Do not overwrite it with a later
result or emit a second provider tool result for the same call. The final result
comes through the worker-completion input, keyed by job/run identity.

### Supervise workers beyond a parent turn

Own workers in the engine, not a component or transient tool future. Natural end
of the parent's provider turn or closing its view must not kill background work.
An explicit session Stop cancels its owned workers even if its foreground turn is
idle; stopping one worker does not stop unrelated workers or the main session.
Fence stale completions and automatic wake-ups after Stop with a generation.
An independent branched conversation is not in this cancellation tree.

If the parent is running, enqueue results for its next safe boundary. If idle,
schedule a serialized follow-up only under the delegation's continuation policy
and current generation; never interrupt another provider attempt or revive a
stopped session. Reconnect hydrates job/status/asks without rerunning tools.

On process restart, preserve completed results and undelivered notifications;
mark nonterminal jobs interrupted. Automatic provider/tool re-execution is out
of scope for the initial async release. Later explicit resume gets a new job-run
identity and verified replay context. A UI refresh is not an execution resume.

### Scope, permissions and budgets

Reuse the parent's permitted delegation boundary, pin worker tools/config/model,
and preserve the current no-nested-delegation rule. Ordinary workers get a complete
task prompt, not implicit full-chat inheritance. Forked worker context is separate
from user branching and can remain deferred until active/bounded fork is ready.

Show pending worker permissions/questions under the owning parent with worker
identity. Replies target their specific job/call. The main agent cannot treat its
own message or another worker's result as permission. Existing tool mutation
ordering and file safety still apply; context isolation is not workspace isolation.

Use a bounded supervisor queue/semaphore, with an explicit small concurrency cap,
per-job provider-step limit, cancellation and usage accounting. Do not copy an
unbounded detached-promise pattern or add a second model runtime. Existing Tokio,
single-writer store, runner and adapters are sufficient.

Expose `GET /sessions/{id}/tasks`, `GET /tasks/{id}` and `POST /tasks/{id}/abort` plus
typed task lifecycle events through generated OpenAPI. A parent-scoped `task_output`
read/wait and `task_stop` tool may inspect/cancel owned jobs; passive completion
notification is the default, not repeated polling. Waiting needs a bound and
returns not-ready/timeout honestly.

## Required fixtures

1. Slow background child: launch receipt arrives, main makes useful progress before
   the child completes, then consumes the real result exactly once.
2. Two independent children: bounded parallel execution and reversed completion order
   retain correct job/result attribution.
3. Foreground child: main cannot use the result until it exists; switching mode at
   launch does not create a persistent thread or nested delegation capability.
4. Worker permission/question: only the right user reply unblocks it; another worker
   and the main can continue; denial and Stop remain terminal for the right job.
5. Parent naturally idle versus explicit Stop: idle keeps background workers alive;
   Stop cancels them, including shell descendants, without restarting the parent.
6. Completion races Stop, archive or parent input: generation fencing prevents stale
   continuation and duplicate notification attachment.
7. Crash/restart before/after result and notification commit: no silent result loss,
   no duplicate delivery, no automatic retry of uncertain effects.
8. UI reload/thread switch: state recovers, old views cannot revive old jobs, completed
   workers do not accumulate in the workspace conversation list.
9. Context/compaction: a running job retains its task context; parent compaction carries
   outstanding job IDs and delivery state without copying its entire transcript.
10. Explicit `/spawn`: review draft at fixed source cutoff, no thread before approval,
    independent conversation afterward; parent Stop cannot cancel that new conversation.
11. Mode resolver: explicit true/false, agent default, omitted default, disabled
    async, skill-context inheritance and workflow-like prose produce deterministic,
    recorded decisions. The words fork/background/workflow alone never change mode.

Use fake providers, controlled tool barriers and worker drains/receipts. Ordinary
tests need no real credentials or paid inference. This document and the canonical
plan add implementation work; none of the proposed async engine behavior is claimed
implemented by the research itself.

## Validation of the plan update

`bun run typecheck` passed. `cargo test --workspace --offline --locked` passed
288 tests, 129 shell plus 159 engine; headless/doc-test targets had zero tests.
Workspace/all-target clippy with warnings denied passed, as did 28 selected native
frontend tests. `git diff --check` passed. These checks validate the existing tree;
the new async acceptance fixtures remain implementation work, not tests already run.
No engine or UI source was edited by this investigation; concurrent user changes
were left intact. The binary was executed only for its version banner.
