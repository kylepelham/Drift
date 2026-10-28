import { childrenOf, taskActive, taskForCall } from "../engine/store";
import { toolInput, toolMetadata as toolMeta } from "../engine/parts";

import type { EngineState, TaskRecord } from "../engine/store";
import type { ToolPart } from "../engine/parts";

/** A `task` or `spawn_thread` call that ran a worker, or is starting one; not one the engine refused outright. */
export function delegates(part: ToolPart) {
    if (part.name !== "task" && part.name !== "spawn_thread") return false;
    if (part.status !== "error" && part.status !== "denied") return true;

    return !!childSessionOf(part);
}

function childSessionOf(part: ToolPart) {
    const sessionId = (toolMeta(part) as { sessionId?: unknown } | undefined)?.sessionId;
    return typeof sessionId === "string" && sessionId ? sessionId : null;
}

export function delegatedChildId(state: EngineState, part: ToolPart) {
    if (!delegates(part)) return null;
    const sessionId = childSessionOf(part);
    if (sessionId) return sessionId;
    if (part.name !== "task") return null;

    // A running call has no metadata yet; the engine's record of this call names its worker.
    const task = taskForCall(state, part.sessionId, part.callId, part.metadata?.taskId);
    if (task) return task.sessionId;

    const input = toolInput(part) as { description?: unknown; subagent_type?: unknown; task_id?: unknown };
    // A continued subagent is named by its task id, which is not a session id.
    if (typeof input?.task_id === "string" && input.task_id) return sessionOfTask(state, part.sessionId, input.task_id);
    if (typeof input?.description !== "string") return null;

    // Before the record arrives, the child is found by the title the engine gives it; `general` is its default agent.
    const agent = typeof input.subagent_type === "string" && input.subagent_type ? input.subagent_type : "general";
    const title = `${input.description} (@${agent} subagent)`;
    const matches = childrenOf(state, part.sessionId).filter((session) => session.title === title);
    return matches.length === 1 ? matches[0].id : null;
}

/** The session a task ran in, or the id itself when it already names a known session. */
function sessionOfTask(state: EngineState, parentId: string, taskId: string) {
    const task = (state.tasks[parentId] ?? []).find((record) => record.id === taskId);
    if (task) return task.sessionId;
    return state.sessions[taskId] ? taskId : null;
}

export type DelegatedTaskStatus = "queued" | "running" | "completed" | "error";

/** A `task` call whose worker runs in the background: the engine's record, or before it arrives, what the call says. */
export function backgroundRun(state: EngineState, part: ToolPart) {
    if (part.name !== "task") return null;

    const metadata = part.metadata;
    const task = taskForCall(state, part.sessionId, part.callId, metadata?.taskId);
    if (task) return task.mode === "background" ? { task } : null;

    const asked = toolInput(part).run_in_background === true;

    return asked || metadata?.background === true || metadata?.mode === "background" ? { task: undefined } : null;
}

export function delegatedTaskStatus(state: EngineState, part: ToolPart, childId: string): DelegatedTaskStatus {
    // This invocation's result stays terminal even when another call resumes the same child.
    if (part.status === "error" || part.status === "denied") return "error";
    if (part.name === "spawn_thread") return part.status === "done" ? "completed" : "running";

    // The engine's record outranks the call: a background call finishes at launch, its worker later.
    const task = taskForCall(state, part.sessionId, part.callId, part.metadata?.taskId);
    if (task) return delegatedRecordStatus(task);

    const terminal = delegatedTerminalState(state, part, childId);
    if (terminal) return terminal;

    return state.errors[childId] ? "error" : "running";
}

function delegatedRecordStatus(task: Pick<TaskRecord, "state">): DelegatedTaskStatus {
    if (task.state === "queued") return "queued";
    if (taskActive(task)) return "running";

    return task.state === "replied" ? "completed" : "error";
}

function delegatedTerminalState(
    state: EngineState,
    part: ToolPart,
    childId: string,
): "completed" | "error" | undefined {
    if (part.status !== "done") return;
    const pattern = new RegExp(
        `^\\s*<task\\s+id=["']${escapeRegExp(childId)}["']\\s+state=["'](running|completed|error)["']`,
    );
    const result = (part.output ?? "").match(pattern)?.[1];
    if (result === "completed" || result === "error") return result;
    const background = part.metadata?.background === true || part.metadata?.mode === "background";
    if (part.name === "task" && result !== "running" && !background) return "completed";

    return followingTaskResult(state.transcripts[part.sessionId] ?? [], part.id, pattern);
}

function followingTaskResult(entries: EngineState["transcripts"][string], partID: string, pattern: RegExp) {
    // A background call ends before its work; take the first result after it.
    let after = false;
    for (const entry of entries) {
        for (const item of entry.parts) {
            if (item.id === partID) after = true;
            if (!after || item.type !== "text") continue;
            const match = item.text.match(pattern)?.[1];
            if (match === "completed" || match === "error") return match;
        }
    }
}

function escapeRegExp(value: string) {
    return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
