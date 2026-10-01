# M3: conversations and subagents

Inspected `1998476160d589cd238f1f8036ae0f752f182eb0` on 2026-09-29. This is a
proposed product clarification following the user's feedback, not an approved
runtime redesign. No runtime code was changed and no visible thread was spawned.

## User's intended distinction

A thread is a user-facing session. The main session can deliberately create one
when the user wants to branch away from the main goal, carrying selected relevant
context. A subagent is a worker on the main session's task. Background execution
does not turn a worker into a separate user conversation.

| Behavior | Subagent | Branched conversation |
| --- | --- | --- |
| Purpose | Help complete the current goal | Pursue a separate goal |
| Context | Self-contained delegation prompt; optional scoped inheritance later | Approved summary/excerpts or explicit transcript fork |
| Output | Result returns to parent | Its own transcript; parent can explicitly check it |
| Lifetime | Owned by the parent's work | Independent after creation |
| Stop | Parent cancellation cascades | Stopping the source conversation does not stop it |
| Sidebar | Temporary active/awaiting-attention indicator at most | Normal persistent session row |
| Completed history | Inspectable from the parent task card | Inspectable as a conversation |
| Creation | Normal permitted delegation | Explicit user action/request, not opportunistic model delegation |

Storage can still use common session/message records. Semantic ownership must not
be inferred only from a shared parent ID or identical UI cards.

## Current implementation

- `tool/task.rs:42-68`: task creates a hidden child with a fresh prompt, waits for
  its result and uses a child abort token. This follows the worker distinction.
- `tool/task.rs:97-126`: model-facing spawn creates a visible sibling, seeds it
  with summary/excerpts/task and starts it independently. It does not clone history.
- `src/engine/actions.ts:327-342`: composer `/spawn` creates a sibling but sends
  only the new task text. It does not transfer parent context. The two entry points
  therefore do not implement the same context-handoff behavior.
- `tool/prompts/spawn_thread.txt` invites autonomous spawning for a discovered bug
  or long-running job, and `SpawnThread::ask` returns none. Main sessions are offered
  that tool; hidden workers cannot use it. This is broader than the user's intent.
- The latest listing change includes all non-archived subagent sessions.
  `src/engine/store.ts:401-405` returns every child without a completion filter,
  and `src/ui/workspaces.tsx:148-157` renders each as a child thread row. Completed
  workers therefore remain in the sidebar. The user's screenshot reflects this
  persistent UI behavior, not an agent still running.

## Recommended M3 adjustment

1. Keep `task` as delegated work. Show its live state and completed result in the
   parent's transcript, with drill-down to its stored transcript when useful.
2. Remove completed subagents from normal sidebar navigation. If active workers
   appear below the parent, show only running/awaiting-attention workers and remove
   their rows on terminal completion. Preserve their records instead of deleting
   task history to solve presentation clutter.
3. Treat `/spawn` as **branch to another conversation**. Require an explicit user
   action/request; the agent can propose a branch but should not create permanent
   sessions merely because it discovers more work.
4. Give UI and model-assisted branching one shared handoff path: new goal plus
   selected summary/excerpts, recorded source session and cutoff, optional user
   review. Creating an empty related session is not context inheritance.
5. Keep true fork separate: copy a stable conversation checkpoint when the user
   wants the same history and another approach. Bounded/active fork is still pending
   in M3; do not claim the current summary-based spawn implements it.
6. A branched conversation has independent Stop, permissions and subsequent
   messages. Its source link is provenance, not a worker ownership relationship.
   Do not automatically merge its replies into the original task.

This preserves the useful session tree and task machinery already written.
Changing the spawning policy and sidebar lifecycle is a product correction, not
an invitation to replace the runner or introduce agent teams.

## Claude Code comparison

Current official docs distinguish normal sessions, session branches and subagents:

- A resumed session retains its session ID/history. `/branch` or the session-fork
  interface creates another session identity from copied conversation history.
- A normal subagent gets a fresh context with its delegation prompt and selected
  configuration, and reports back within the parent session. Foreground/background
  are execution modes, not the distinction between a conversation and a worker.
- A forked subagent can inherit conversation context but remains delegated work
  whose result returns to the parent. Current docs call this `/subtask`; command
  names and behavior changed across versions.
- Agent teams are a separate coordinated-session feature, not required for this
  ordinary branching/delegation model.

These are current documentation findings, not claims that the inspected older
Claude Code 2.1.85 has every newer command. Relevant sources:
<https://code.claude.com/docs/en/sessions>,
<https://code.claude.com/docs/en/sub-agents>,
<https://code.claude.com/docs/en/agent-sdk/sessions> and
<https://code.claude.com/docs/en/agent-teams>.

Conversation isolation also does not imply filesystem isolation. Sessions or
workers using the same workspace see each other's file changes unless separate
worktrees are explicitly used.

## Validation and status

Current workspace tests pass: 285 total, 129 shell plus 156 engine. Typecheck and
26 selected native frontend tests pass. Existing tests verify hidden-task results,
parent cancellation, independent sibling creation and refusal of nested delegation.
They validate the implemented behavior, not the product semantics requested here.

M3 currently has delegation/spawn work landed. Fork, move, compaction, revert and
other lifecycle items remain pending. This is not a full M3 sign-off. Before
accepting the adjusted behavior, add tests for completed-worker sidebar removal,
source-context handoff through the composer, explicit branch authorization and
independent branch cancellation. Canonical plan/checklist remain unchanged pending
agreement on these product changes.
