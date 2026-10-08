import { clearQuestionDraft } from "../state/question-drafts";
import { sessionInWorkspace } from "./sessions";
import { questionForCard } from "./questions";
import { adaptPart } from "./native/adapt";
import { produce } from "solid-js/store";
import {
    bumpRevision,
    messageRevisionKey,
    normalizeDir,
    pruneSessionRevisions,
    putSession,
    recordLink,
    revisionAdvanced,
    sessionRevisionKey,
    spawnLink,
    statusRevisionKey,
    type EngineState,
    type ModelRef,
    type Notice,
    type Permission,
    type QuestionRequest,
} from "./store";

import type { Session, WorkspaceIndex } from "./sessions";
import type { SetStoreFunction } from "solid-js/store";
import type { Part, SessionStatus } from "./shapes";
import type { components } from "./native/types";
import type { Message } from "./messages";

type SetEngineState = SetStoreFunction<EngineState>;
type Event = components["schemas"]["Event"];

export function reduce(
    set: SetEngineState,
    event: Event,
    directory?: string,
    reconcile?: (sessionID: string) => void,
    workspaces: WorkspaceIndex = { path: () => undefined, id: () => undefined },
) {
    if (reduceSessionEvent(set, event, workspaces)) return;

    reduceContentEvent(set, event, directory, reconcile);
}

function reduceSessionEvent(set: SetEngineState, event: Event, workspaces: WorkspaceIndex) {
    switch (event.type) {
        case "session.created":
        case "session.updated":
            putSession(set, sessionInWorkspace(event.session, workspaces));
            return true;
        case "session.deleted":
            set(produce((draft) => purgeSession(draft, event.sessionId)));
            return true;
        case "session.status":
            updateStatus(set, event.sessionId, event.status === "running" ? { type: "busy" } : { type: "idle" });
            return true;
        case "session.retry":
            updateStatus(set, event.sessionId, {
                type: "retry",
                attempt: event.attempt,
                message: event.message,
                next: event.nextAt,
            });
            return true;
        default:
            return false;
    }
}

function reduceContentEvent(
    set: SetEngineState,
    event: Event,
    directory?: string,
    reconcile?: (sessionID: string) => void,
) {
    switch (event.type) {
        case "message.created":
        case "message.updated":
            return upsertMessage(set, event.message);
        case "message.removed":
            return dropMessage(set, event.sessionId, event.messageId);
        case "part.created":
        case "part.updated":
            return upsertPart(set, adaptPart(event.part));
        case "part.delta":
            return appendPartDelta(set, event, reconcile);
        case "permission.asked":
            return addPermission(set, { ...event.request, directory: "" });
        case "permission.replied":
            return dropPermission(set, event.sessionId, event.requestId);
        case "question.asked":
            return addQuestion(set, { ...questionForCard(event.request), directory });
        case "question.replied":
            return dropQuestion(set, event.sessionId, event.requestId);
        case "todo.updated":
            return set("todos", event.sessionId, event.todos);
        case "plugin.notice":
            return pushNotice(set, {
                id: `notice-${Date.now()}-${noticeSequence++}`,
                title: `${event.plugin}: ${event.title}`,
                message: event.body,
                variant: noticeVariant(event.tone),
                created: Date.now(),
                duration: 8000,
            });
    }
}

function updateStatus(set: SetEngineState, sessionID: string, status: SessionStatus) {
    set(
        produce((draft) => {
            draft.status[sessionID] = status;
            bumpRevision(draft, statusRevisionKey(sessionID));
            if (status.type === "idle") clearLiveTools(draft, sessionID);
        }),
    );

    if (status.type !== "idle") clearError(set, sessionID);
}

// The session revision bump outlives the purge so an in-flight snapshot taken before the
// deletion cannot resurrect the session.
function purgeSession(draft: EngineState, id: string) {
    delete draft.sessions[id];
    delete draft.transcripts[id];
    delete draft.loaded[id];
    delete draft.permissions[id];
    delete draft.questions[id];
    delete draft.todos[id];
    delete draft.tasks[id];
    delete draft.status[id];
    delete draft.activity[id];
    delete draft.errors[id];
    delete draft.sessionModels[id];
    delete draft.cursors[id];
    clearLiveTools(draft, id);
    pruneSessionRevisions(draft, id);
    bumpRevision(draft, sessionRevisionKey(id));
}

// Applies a session-list snapshot. Sessions whose revision advanced while the request was in
// flight keep their live state. When `scope` is present the snapshot is authoritative and
// complete for that directory, so sessions absent from it are purged; partial or failed
// snapshots must never pass a scope.
export function applySessionSnapshot(
    set: SetEngineState,
    input: { sessions: Session[]; captured: Record<string, number>; scope?: { directory: string } | { all: true } },
) {
    const ids = new Set(input.sessions.map((info) => info.id));
    const all = input.scope && "all" in input.scope;
    const dir = input.scope && "directory" in input.scope ? normalizeDir(input.scope.directory) : undefined;
    set(
        produce((draft) => {
            const advanced = (id: string) => revisionAdvanced(draft.revisions, input.captured, sessionRevisionKey(id));
            for (const info of input.sessions) {
                if (advanced(info.id)) continue;
                draft.sessions[info.id] = { revert: undefined, ...info };
                // Applying a snapshot advances the session so an older overlapping snapshot (a reconnect
                // flap fires two hydrates) can neither downgrade nor purge what this one established.
                bumpRevision(draft, sessionRevisionKey(info.id));
                const model = info.model;
                if (model) draft.sessionModels[info.id] = { providerID: model.provider, modelID: model.model };
            }
            if (!input.scope) return;
            for (const session of Object.values(draft.sessions)) {
                if (ids.has(session.id) || advanced(session.id)) continue;
                if (!all && normalizeDir(session.directory) !== dir) continue;
                // Scoped listings exclude engine-archived sessions, so their absence is not a deletion.
                // Purging them here would delete-and-reload archived transcripts on every hydration.
                if (!all && session.archivedAt) continue;
                purgeSession(draft, session.id);
            }
        }),
    );
}

// Applies a status snapshot for the given sessions, skipping any whose status a live event
// already moved past the capture point.
export function applyStatusSnapshot(
    set: SetEngineState,
    input: { sessions: Session[]; statuses: Record<string, SessionStatus>; captured: Record<string, number> },
) {
    set(
        produce((draft) => {
            for (const session of input.sessions) {
                if (!draft.sessions[session.id]) continue;
                if (revisionAdvanced(draft.revisions, input.captured, statusRevisionKey(session.id))) continue;
                const status = input.statuses[session.id] ?? { type: "idle" as const };
                draft.status[session.id] = status;
                if (status.type === "idle") clearLiveTools(draft, session.id);
            }
        }),
    );
}

function clearLiveTools(draft: EngineState, sessionID: string) {
    for (const [partID, owner] of Object.entries(draft.liveTools))
        if (owner === sessionID) delete draft.liveTools[partID];
}

function clearError(set: SetEngineState, sessionID: string) {
    set(
        produce((draft) => {
            delete draft.errors[sessionID];
        }),
    );
}

function upsertMessage(set: SetEngineState, info: Message) {
    set(
        produce((draft) => {
            if (info.role === "assistant")
                draft.sessionModels[info.sessionId] = {
                    providerID: info.model?.provider ?? "",
                    modelID: info.model?.model ?? "",
                    messageId: info.id,
                };
            const list = draft.loaded[info.sessionId] ? draft.transcripts[info.sessionId] : undefined;
            if (!list) return;
            bumpRevision(draft, messageRevisionKey(info.sessionId, info.id));
            const index = list.findIndex((entry) => entry.info.id === info.id);
            if (index >= 0) list[index].info = info;
            else list.push({ info, parts: [] });
        }),
    );
}

function dropMessage(set: SetEngineState, sessionID: string, messageID: string) {
    set(
        produce((draft) => {
            const list = draft.transcripts[sessionID];
            if (!list) return;
            bumpRevision(draft, messageRevisionKey(sessionID, messageID));
            draft.transcripts[sessionID] = list.filter((entry) => entry.info.id !== messageID);
        }),
    );
}

function upsertPart(set: SetEngineState, part: Part) {
    const link = spawnLink(part);
    if (link) recordLink(link);
    set(
        produce((draft) => {
            if (link) draft.links[link.child] = link.parent;
            if (link && part.type === "tool") {
                const metadata = (("metadata" in part.state ? part.state.metadata : undefined) ?? part.metadata) as
                    { model?: ModelRef } | undefined;
                if (metadata?.model) draft.sessionModels[link.child] = metadata.model;
            }
            if (part.type === "tool") {
                trackActivity(draft, part);
                if (part.state.status === "pending" || part.state.status === "running")
                    draft.liveTools[part.id] = part.sessionID;
                else delete draft.liveTools[part.id];
            }
            const entry = draft.transcripts[part.sessionID]?.find((item) => item.info.id === part.messageID);
            if (!entry) return;
            bumpRevision(draft, messageRevisionKey(part.sessionID, part.messageID));
            const index = entry.parts.findIndex((existing) => existing.id === part.id);
            if (index >= 0) entry.parts[index] = reconcilePart(entry.parts[index]!, part);
            else entry.parts.push(part);
        }),
    );
}

function reconcilePart(existing: Part, incoming: Part) {
    if (existing.type !== incoming.type || (incoming.type !== "text" && incoming.type !== "reasoning")) return incoming;
    if (existing.type !== "text" && existing.type !== "reasoning") return incoming;
    // REST hydration can race an older initial part.updated frame whose text is still empty. Keep
    // the hydrated prefix so following deltas append to it; completed/non-empty updates remain
    // authoritative and can still replace the part normally. This relies on the engine allocating a
    // fresh part ID per streamed attempt (PartID.ascending on every text-start): a same-ID reset to
    // a shorter prefix is therefore always the stale frame, never a legitimate rewrite.
    if (
        incoming.time?.end === undefined &&
        existing.text.length > incoming.text.length &&
        existing.text.startsWith(incoming.text)
    ) {
        return { ...incoming, text: existing.text };
    }
    return incoming;
}

type PartDeltaRef = Extract<Event, { type: "part.delta" }>;

/** The field after a delta: a snapshot that already holds it is left alone, one cut short is completed. */
export function withDelta(current: string, delta: string, offset?: number) {
    if (offset === undefined) return current + delta;
    if (offset > current.length) return current;
    if (current.length >= offset + delta.length) return current;
    return current.slice(0, offset) + delta;
}

function appendPartDelta(set: SetEngineState, ref: PartDeltaRef, reconcile?: (sessionID: string) => void) {
    let gap = false;
    set(
        produce((draft) => {
            const entry = draft.transcripts[ref.sessionId]?.find((item) => item.info.id === ref.messageId);
            const index = entry?.parts.findIndex((item) => item.id === ref.partId) ?? -1;
            if (!entry) return;
            if (index < 0) {
                gap = true;
                return;
            }
            const part = entry.parts[index]!;
            if (part.type !== "text" && part.type !== "reasoning") return;

            if (ref.offset > part.text.length) {
                gap = true;
                return;
            }

            const next = withDelta(part.text, ref.delta, ref.offset);
            if (next !== part.text) {
                bumpRevision(draft, messageRevisionKey(ref.sessionId, ref.messageId));
                entry.parts[index] = { ...part, text: next };
            }
        }),
    );
    if (gap) reconcile?.(ref.sessionId);
}

function trackActivity(draft: EngineState, part: Part & { type: "tool" }) {
    const entry = draft.activity[part.sessionID] ?? { tools: 0, lastPartId: "" };
    if (entry.lastPartId !== part.id) {
        entry.tools += 1;
        entry.lastPartId = part.id;
    }
    entry.current = part.state.status === "completed" || part.state.status === "error" ? undefined : part.tool;
    draft.activity[part.sessionID] = entry;
}

function addQuestion(set: SetEngineState, question: QuestionRequest) {
    set(
        produce((draft) => {
            const list = draft.questions[question.sessionId] ?? [];
            if (!list.some((existing) => existing.id === question.id)) list.push(question);
            draft.questions[question.sessionId] = list;
        }),
    );
}

function dropQuestion(set: SetEngineState, sessionID: string, requestID: string) {
    clearQuestionDraft(requestID);
    set(
        produce((draft) => {
            const list = draft.questions[sessionID];
            if (list) draft.questions[sessionID] = list.filter((question) => question.id !== requestID);
        }),
    );
}

function addPermission(set: SetEngineState, permission: Permission) {
    set(
        produce((draft) => {
            const list = draft.permissions[permission.sessionId] ?? [];
            if (!list.some((existing) => existing.id === permission.id)) list.push(permission);
            draft.permissions[permission.sessionId] = list;
        }),
    );
}

function dropPermission(set: SetEngineState, sessionID: string, permissionID: string) {
    set(
        produce((draft) => {
            const list = draft.permissions[sessionID];
            if (list) draft.permissions[sessionID] = list.filter((permission) => permission.id !== permissionID);
        }),
    );
}

let noticeSequence = 0;

function noticeVariant(value: unknown): Notice["variant"] {
    return value === "success" || value === "warning" || value === "error" ? value : "info";
}

export function pushNotice(set: SetEngineState, notice: Notice) {
    set(
        produce((draft) => {
            draft.notices = [
                ...draft.notices.filter(
                    (item) =>
                        item.id !== notice.id &&
                        (item.title !== notice.title ||
                            item.message !== notice.message ||
                            item.variant !== notice.variant),
                ),
                notice,
            ].slice(-6);
        }),
    );
}
