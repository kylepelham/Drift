import { createStore, produce, type SetStoreFunction } from "solid-js/store";
import { hiddenParent } from "./sessions";
import { messageModel } from "./messages";
import { promptPartText } from "./parts";

import type { ModelInfo, ProviderInfo } from "./catalog";
import type { QuestionRequest } from "./questions";
import type { components } from "./native/types";
import type { Session } from "./sessions";
import type { Message } from "./messages";
import type { Part } from "./parts";
import type {
    McpServerConfig,
    McpServerConfigView,
    McpServerStatus,
    PermissionRule,
    TaskRecord,
} from "./native/client";
export type { McpServerConfig, McpServerConfigView, McpServerStatus, TaskRecord };
export type { QuestionInfo, QuestionRequest } from "./questions";
export type Permission = components["schemas"]["PermissionRequest"] & { directory?: string };
type Todo = components["schemas"]["Todo"];
export type Connection = "idle" | "connecting" | "online" | "offline";

export type { ModelInfo, ProviderInfo } from "./catalog";
export type ModelRef = { providerID: string; modelID: string };
/** An agent as the engine resolved it for the workspace, Settings overrides applied. */
export type AgentInfo = {
    name: string;
    description: string;
    mode: "primary" | "subagent" | "all";
    /** Engine actions (titles, compaction) and agents marked `hidden`: never picked in the composer. */
    hidden: boolean;
    builtIn: boolean;
    prompt?: string;
    model?: ModelRef;
    /** Tool names it may use; empty means every tool. */
    tools: string[];
    /** Its own step limit, in place of the workspace's. */
    steps?: number;
    permissions?: PermissionRule[];
    variant?: string;
    /** Why the engine refuses to run it (a broken file or override); other agents are unaffected. */
    problem?: string;
};
export type SessionStatus =
    { type: "idle" | "busy" } | { type: "retry"; attempt: number; message: string; next: number };

export type CommandInfo = Pick<components["schemas"]["Command"], "name" | "description" | "template"> & {
    usage?: string;
    subcommands?: { name: string; description: string; usage?: string }[];
};
export type MessageEntry = { info: Message; parts: Part[] };

export function interruptStaleTools(
    entries: MessageEntry[],
    liveTools: Readonly<Record<string, string>>,
    error = "Interrupted",
) {
    return entries.map((entry) => {
        let changed = false;
        const parts = entry.parts.map((part) => {
            if (part.type !== "tool_call" || (part.status !== "pending" && part.status !== "running")) return part;
            if (liveTools[part.id] === part.sessionId) return part;
            changed = true;
            const completed = entry.info.finishedAt;
            const start = part.startedAt ?? undefined;
            return {
                ...part,
                status: "error" as const,
                output: error,
                finishedAt: start === undefined ? undefined : Math.max(start, completed ?? start),
            };
        });
        return changed ? { ...entry, parts } : entry;
    });
}

export function messageText(entry: MessageEntry) {
    return entry.parts
        .flatMap((part) => {
            const text = promptPartText(part);
            return text === undefined ? [] : [text];
        })
        .join("\n");
}

// Engine IDs are not chronologically sortable (the embedded timestamp wraps), so order by time first.
export function compareMessages(a: MessageEntry, b: MessageEntry) {
    return (a.info.createdAt ?? 0) - (b.info.createdAt ?? 0) || a.info.id.localeCompare(b.info.id);
}

function messageBoundary(entries: MessageEntry[], id: string | undefined, entry: MessageEntry) {
    const boundary = id ? entries.find((candidate) => candidate.info.id === id) : undefined;
    if (!boundary) return id ? entry.info.id.localeCompare(id) : undefined;
    return compareMessages(entry, boundary);
}

export function previousUserMessage(entries: MessageEntry[], before?: string) {
    return entries
        .filter((entry) => entry.info.role === "user" && (messageBoundary(entries, before, entry) ?? -1) < 0)
        .sort(compareMessages)
        .at(-1);
}

export function nextUserMessage(entries: MessageEntry[], after: string) {
    return entries
        .filter((entry) => entry.info.role === "user" && (messageBoundary(entries, after, entry) ?? 1) > 0)
        .sort(compareMessages)[0];
}

type SessionActivity = { tools: number; lastPartId: string; current?: string };

export type Notice = {
    id: string;
    title?: string;
    message: string;
    variant: "info" | "success" | "warning" | "error";
    created: number;
    duration: number;
};

export type EngineState = {
    connection: Connection;
    directory: string;
    bootstrappedDirectory: string;
    sessionSnapshotDirectory: string;
    sessionSnapshotAll: boolean;
    sessionSnapshotEpoch: number;
    providerSnapshotEpoch: number;
    sessions: Record<string, Session>;
    status: Record<string, SessionStatus>;
    transcripts: Record<string, MessageEntry[]>;
    loaded: Record<string, boolean>;
    permissions: Record<string, Permission[]>;
    questions: Record<string, QuestionRequest[]>;
    todos: Record<string, Todo[]>;
    /** Workers each session launched, keyed by the launching session, oldest first. */
    tasks: Record<string, TaskRecord[]>;
    providers: ProviderInfo[];
    /** The engine's MCP servers by name: the only place their definition and state live. */
    mcpServers: Record<string, McpServerStatus>;
    connected: string[];
    defaultModels: Record<string, string>;
    agents: AgentInfo[];
    commands: CommandInfo[];
    errors: Record<string, string>;
    sessionModels: Record<string, ModelRef & { messageId?: string }>;
    notices: Notice[];
    links: Record<string, string>;
    activity: Record<string, SessionActivity>;
    liveTools: Record<string, string>;
    cursors: Record<string, string | null>;
    revisions: Record<string, number>;
    version: string;
    nativeVersion: string;
    nativeOnline: boolean;
    startupError: string;
    engineError: string;
    engineRestarting: boolean;
    /** Every session answers its own asks (Settings), as the engine has it. */
    autoAcceptAll: boolean;
};

let storedLinks: Record<string, string> | undefined;

function loadLinks(): Record<string, string> {
    if (storedLinks) return storedLinks;
    try {
        storedLinks = JSON.parse(localStorage.getItem("drift.links") ?? "{}") as Record<string, string>;
    } catch {
        storedLinks = {};
    }
    return storedLinks;
}

export function recordLink(link: { child: string; parent: string }) {
    const links = loadLinks();
    if (links[link.child] === link.parent) return;
    links[link.child] = link.parent;
    try {
        localStorage.setItem("drift.links", JSON.stringify(links));
    } catch {
        // Storage failure must not terminate the global engine event pump.
    }
}

export function createEngineState() {
    return createStore<EngineState>({
        connection: "idle",
        directory: "",
        bootstrappedDirectory: "",
        sessionSnapshotDirectory: "",
        sessionSnapshotAll: false,
        sessionSnapshotEpoch: 0,
        providerSnapshotEpoch: 0,
        sessions: {},
        status: {},
        transcripts: {},
        loaded: {},
        permissions: {},
        questions: {},
        todos: {},
        tasks: {},
        providers: [],
        mcpServers: {},
        connected: [],
        defaultModels: {},
        agents: [],
        commands: [],
        errors: {},
        sessionModels: {},
        notices: [],
        links: { ...loadLinks() },
        activity: {},
        liveTools: {},
        startupError: "",
        engineError: "",
        engineRestarting: false,
        autoAcceptAll: false,
        cursors: {},
        revisions: {},
        version: "",
        nativeVersion: "",
        nativeOnline: false,
    });
}

/** The engine knows which thread spawned which; that beats links inferred from tool parts. */
function linkSpawned(links: Record<string, string>, info: Session) {
    const parent = info.visibility === "sibling" ? info.parentId : undefined;
    if (!parent) return;
    links[info.id] = parent;
    recordLink({ child: info.id, parent });
}

// Store sets merge, so a revert marker the engine dropped must clear explicitly.
export function putSession(set: SetStoreFunction<EngineState>, info: Session) {
    set(
        produce((draft) => {
            draft.sessions[info.id] = { revert: undefined, ...info };
            linkSpawned(draft.links, info);
            const model = info.model;
            if (model) draft.sessionModels[info.id] = { providerID: model.provider, modelID: model.model };
            bumpRevision(draft, sessionRevisionKey(info.id));
        }),
    );
}

// Monotonic counters bumped by every live reduction that touches the keyed slice. Snapshot writes
// compare them against a capture taken before the HTTP request started, so state that raced ahead
// of the snapshot is never overwritten by it.
export function sessionRevisionKey(sessionID: string) {
    return `session\0${sessionID}`;
}

export function statusRevisionKey(sessionID: string) {
    return `status\0${sessionID}`;
}

export function messageRevisionKey(sessionID: string, messageID: string) {
    return `message\0${sessionID}\0${messageID}`;
}

export function bumpRevision(draft: EngineState, key: string) {
    draft.revisions[key] = (draft.revisions[key] ?? 0) + 1;
}

export function captureRevisions(state: EngineState): Record<string, number> {
    return { ...state.revisions };
}

export function revisionAdvanced(current: Record<string, number>, captured: Record<string, number>, key: string) {
    return (current[key] ?? 0) !== (captured[key] ?? 0);
}

// The session revision survives so an in-flight snapshot cannot resurrect a purged session.
export function pruneSessionRevisions(draft: EngineState, sessionID: string) {
    delete draft.revisions[statusRevisionKey(sessionID)];
    const prefix = `message\0${sessionID}\0`;
    for (const key of Object.keys(draft.revisions)) if (key.startsWith(prefix)) delete draft.revisions[key];
}

// Merges a transcript snapshot with what the event stream did while the request was in flight:
// the snapshot is authoritative for untouched messages, live state wins for touched ones.
export function mergeTranscriptSnapshot(
    live: MessageEntry[] | undefined,
    snapshot: MessageEntry[],
    sessionID: string,
    captured: Record<string, number>,
    revisions: Record<string, number>,
) {
    const advanced = (messageID: string) =>
        revisionAdvanced(revisions, captured, messageRevisionKey(sessionID, messageID));
    const liveById = new Map((live ?? []).map((entry) => [entry.info.id, entry]));
    const snapshotIds = new Set(snapshot.map((entry) => entry.info.id));
    const merged = snapshot.flatMap((snapshotEntry) => {
        const entry = snapshotEntry;
        const current = liveById.get(entry.info.id);
        if (!advanced(entry.info.id)) {
            // Reuse the live object when the content is unchanged: transcript rows are referentially
            // keyed, so handing the UI a fresh-but-identical object would remount every visible row
            // (a full-transcript flash on each reconnect hydration).
            return [current && JSON.stringify(current) === JSON.stringify(entry) ? current : entry];
        }
        return current ? [withSnapshotParts(current, entry)] : [];
    });
    for (const entry of live ?? []) if (advanced(entry.info.id) && !snapshotIds.has(entry.info.id)) merged.push(entry);
    return merged.sort(compareMessages);
}

// Repair missing parts and shorter prefixes without replacing newer live metadata.
function withSnapshotParts(current: MessageEntry, snapshot: MessageEntry): MessageEntry {
    const byId = new Map(snapshot.parts.map((part) => [part.id, part]));
    let changed = false;
    const parts = current.parts.map((part) => {
        if (part.type !== "text" && part.type !== "reasoning") return part;
        const incoming = byId.get(part.id);
        if (
            incoming?.type !== part.type ||
            incoming.text.length <= part.text.length ||
            !incoming.text.startsWith(part.text)
        )
            return part;
        changed = true;
        return { ...part, text: incoming.text };
    });
    const present = new Set(parts.map((part) => part.id));
    for (const part of snapshot.parts) {
        if (present.has(part.id)) continue;
        present.add(part.id);
        parts.push(part);
        changed = true;
    }
    return changed ? { ...current, parts: parts.sort((a, b) => a.id.localeCompare(b.id)) } : current;
}

export function modelInfo(state: EngineState, ref: ModelRef | null): ModelInfo | undefined {
    if (!ref) return undefined;
    return state.providers.find((p) => p.id === ref.providerID)?.models[ref.modelID];
}

function tokenCount(usage: components["schemas"]["Usage"]) {
    return usage.input + usage.output + usage.cacheRead + usage.cacheWrite;
}

// Ceiling on how much of the context window is set aside for the model's own reply, and the slice
// of that reserved for compaction headroom. The reply cap mirrors MAX_REPLY_TOKENS in
// crates/drift-engine/src/llm/catalog.rs; change both or the meter drifts from real compaction.
const maxOutputTokens = 32000;
const compactionReserveTokens = 20000;
const percentScale = 100;

/** Mirrors the engine's `Model::reply_room`: the output limit, else a quarter of a known window; never over half a known window or the cap. */
export function replyRoom(output: number, context: number) {
    const room = output || (context ? Math.floor(context / 4) : maxOutputTokens);
    return Math.min(room, context ? Math.floor(context / 2) : room, maxOutputTokens);
}

/** Below this window the system prompt and tool schemas leave little room for work; mirrors `SMALL_CONTEXT`. */
export const smallContextTokens = 16_384;

// Mirrors the engine's `overflowing` (session/compaction.rs) so the meter predicts the same compaction point.
// Limits come from the model the next prompt would use; token counts from the last reply.
export function contextStats(state: EngineState, sessionId: string, modelRef?: ModelRef | null) {
    const entries = state.transcripts[sessionId] ?? [];
    // Usage from before the latest compaction no longer describes what the model sees.
    const newestFirst = [...entries].reverse();
    const summaryAt = newestFirst.findIndex((entry) => !!entry.info.summary);
    const sinceSummary = summaryAt < 0 ? newestFirst : newestFirst.slice(0, summaryAt);
    const last = sinceSummary.find((entry) => {
        return entry.info.role === "assistant" && tokenCount(entry.info.usage) > 0;
    });
    if (!last) return null;
    const tokens = last.info.usage;
    const count = tokenCount(tokens);
    const model = modelInfo(state, modelRef ?? null) ?? modelInfo(state, messageModel(last.info));
    const limits = (model?.limit ?? {}) as { context?: number; output?: number; input?: number };
    const context = limits.context ?? 0;
    if (!context || !count) return null;
    const maxOutput = replyRoom(limits.output ?? 0, context);
    const reserved = Math.min(compactionReserveTokens, maxOutput);
    // Mirrors `Model::compaction_point`: an input cap counts only when it is below the window.
    const usable = usableContext(limits.input, context, reserved, maxOutput);
    return {
        count,
        context,
        percent: Math.min(percentScale, Math.round((count / context) * percentScale)),
        untilCompaction: Math.max(0, usable - count),
        // The engine keeps cost per message (replies and compaction summaries), not per session.
        cost: entries.reduce((sum, entry) => sum + ((entry.info as { cost?: number }).cost ?? 0), 0),
    };
}

function usableContext(input: number | undefined, context: number, reserved: number, maxOutput: number) {
    if (input && input < context) return Math.max(0, input - reserved);

    return Math.max(0, context - maxOutput);
}

export function spawnLink(part: Part): { child: string; parent: string } | undefined {
    if (part.type !== "tool_call" || (part.name !== "task" && part.name !== "spawn_thread")) return;
    const meta = part.metadata;
    if (!meta?.sessionId) return;
    return { child: meta.sessionId, parent: part.sessionId };
}

export function taskActive(task: Pick<TaskRecord, "state">) {
    return task.state === "queued" || task.state === "running";
}

// A task only moves forward (queued, running, ended, held, delivered), so the further one is the newer.
function taskProgress(task: TaskRecord) {
    const stage = taskStage(task.state);
    return stage + (task.held ? 1 : 0) + (task.delivered ? 2 : 0);
}

function taskStage(state: TaskRecord["state"]) {
    if (state === "queued") return 0;
    if (state === "running") return 1;

    return 2;
}

/** Folds task records in; an older copy (a snapshot that raced an event) never replaces a newer one. */
export function mergeTasks(current: readonly TaskRecord[] | undefined, incoming: readonly TaskRecord[]) {
    const byId = new Map((current ?? []).map((task) => [task.id, task]));
    for (const task of incoming) {
        const known = byId.get(task.id);
        if (!known || taskProgress(task) >= taskProgress(known)) byId.set(task.id, task);
    }
    return [...byId.values()].sort((a, b) => a.createdAt - b.createdAt || a.id.localeCompare(b.id));
}

export function putTasks(
    set: SetStoreFunction<EngineState>,
    state: EngineState,
    parentId: string,
    tasks: readonly TaskRecord[],
) {
    set("tasks", parentId, mergeTasks(state.tasks[parentId], tasks));
}

/** The task a `task` tool call launched, when the engine has reported it. */
export function taskForCall(state: EngineState, sessionId: string, callId: string | undefined, taskId: unknown) {
    const tasks = state.tasks[sessionId] ?? [];
    return tasks.find(
        (task) =>
            (typeof taskId === "string" && task.id === taskId) || (callId !== undefined && task.callId === callId),
    );
}

/** The newest task that ran in a worker's session; tasks are kept oldest first. */
export function taskForWorker(state: EngineState, sessionId: string) {
    const parentId = hiddenParent(state.sessions[sessionId]);
    return parentId ? (state.tasks[parentId] ?? []).filter((task) => task.sessionId === sessionId).at(-1) : undefined;
}

/** A task's own run as tool timing: from launch until it ended, not the launching call's instant. */
export function taskTiming(task: Pick<TaskRecord, "state" | "createdAt" | "finishedAt">) {
    // A queued worker has not started, so it has no running time to show yet.
    if (task.state === "queued") return { status: "queued", time: {} };

    return {
        status: taskActive(task) ? "running" : "completed",
        time: { start: task.createdAt, end: task.finishedAt ?? undefined },
    };
}

/** The worker session's newest task is waiting for a free background slot. */
export function workerQueued(state: EngineState, sessionId: string) {
    return taskForWorker(state, sessionId)?.state === "queued";
}

/** The model, agent and reasoning level the engine saved on a session: what its newest prompt chose. */
export function savedChoice(
    state: EngineState,
    id: string | null | undefined,
): { agent?: string; variant?: string | null; model?: ModelRef } {
    const session = id ? state.sessions[id] : undefined;
    if (!session) return {};
    const model = session.model ? { model: { providerID: session.model.provider, modelID: session.model.model } } : {};
    return { agent: session.agent, variant: session.variant ?? null, ...model };
}

export function sessionBusy(state: EngineState, id: string) {
    const status = state.status[id]?.type;
    return status === "busy" || status === "retry";
}

export function normalizeDir(path: string) {
    return path.replaceAll("\\", "/").replace(/\/+$/, "").toLowerCase();
}

export function sessionsFor(state: EngineState, directory: string) {
    const dir = normalizeDir(directory);
    return Object.values(state.sessions)
        .filter((session) => !hiddenParent(session) && normalizeDir(session.directory) === dir)
        .sort((a, b) => b.updatedAt - a.updatedAt);
}

export function childrenOf(state: EngineState, parentId: string) {
    return Object.values(state.sessions)
        .filter((session) => hiddenParent(session) === parentId)
        .sort((a, b) => a.createdAt - b.createdAt);
}

const providerPriority = ["anthropic", "openai", "opencode", "github-copilot", "google", "zai", "xai"];

export function resolveModel(state: EngineState, pref: ModelRef | null): ModelRef | null {
    if (
        pref &&
        (state.connection !== "online" || state.connected.includes(pref.providerID)) &&
        state.providers.some((p) => p.id === pref.providerID && pref.modelID in p.models)
    )
        return pref;
    const connected = state.providers.filter((p) => state.connected.includes(p.id));
    const pool = availableProviders(state, connected);
    const rank = (id: string) => {
        const index = providerPriority.indexOf(id);
        return index < 0 ? providerPriority.length : index;
    };
    for (const provider of [...pool].sort((a, b) => rank(a.id) - rank(b.id))) {
        const usable = Object.values(provider.models);
        if (!usable.length) continue;
        const preferred = provider.models[state.defaultModels[provider.id] ?? ""];
        const model = preferred ?? usable[0];
        return { providerID: provider.id, modelID: model.id };
    }
    return null;
}

function availableProviders(state: EngineState, connected: ProviderInfo[]) {
    if (state.connection === "online" || connected.length) return connected;

    return state.providers;
}
