import { childrenOf, taskActive, taskForCall } from "../engine/store";
import { toolInput, toolMetadata as toolMeta } from "../engine/parts";

import type { EngineState, TaskRecord } from "../engine/store";
import type { ToolPart } from "../engine/parts";

export function delegatedTaskClickPolicy(status: DelegatedTaskStatus | null, childId: string | null) {
    return childId && status === "running" ? "navigate" : "expand";
}

export function delegatedChildId(state: EngineState, part: ToolPart) {
    if (part.name !== "task" && part.name !== "spawn_thread") return null;
    const sessionId = (toolMeta(part) as { sessionId?: unknown } | undefined)?.sessionId;
    if (typeof sessionId === "string" && sessionId) return sessionId;
    if (part.name !== "task") return null;

    const input = toolInput(part) as { description?: unknown; subagent_type?: unknown; task_id?: unknown };
    if (typeof input?.task_id === "string" && input.task_id) return input.task_id;
    if (typeof input?.description !== "string" || typeof input.subagent_type !== "string") return null;

    // Parallel tasks can create their child before the running tool part persists its session metadata.
    const title = `${input.description} (@${input.subagent_type} subagent)`;
    const matches = childrenOf(state, part.sessionId).filter((session) => session.title === title);
    return matches.length === 1 ? matches[0].id : null;
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
