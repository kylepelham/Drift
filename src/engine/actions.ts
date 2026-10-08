import { errorMessage, type ActionContext, type NoticeInput } from "./actions-context";
import { applySessionSnapshot, applyStatusSnapshot, pushNotice } from "./events";
import { EngineError, maxRequestBytes, type Client } from "./native/client";
import { produce, type SetStoreFunction } from "solid-js/store";
import { createProviderActions } from "./actions-providers";
import { createSessionActions } from "./actions-sessions";
import { formatAttachmentBytes } from "../attachments";
import { createConfigActions } from "./actions-config";
import { createAskActions } from "./actions-asks";
import { sessionInWorkspace } from "./sessions";
import { untrack } from "solid-js";
import { t } from "../state/i18n";
import {
    captureRevisions,
    compareMessages,
    interruptStaleTools,
    mergeTranscriptSnapshot,
    putSession,
    putTasks,
    savedChoice,
    type EngineState,
    type MessageEntry,
    type ModelRef,
} from "./store";

// Engine actions own HTTP requests; message, part and catalog views still use the adapter.
import type { Session, WorkspaceIndex } from "./sessions";
import type { components } from "./native/types";

type NativeMessageWithParts = components["schemas"]["MessageWithParts"];

type PromptFile = {
    filename?: string;
    mime: string;
    url: string;
    source?: { type: "file"; path: string; text: { value: string; start: number; end: number } };
};
/**
 * `variant` null asks for the model's default level; left out, the session keeps its own (as for a level the model does
 * not offer).
 */
export type PromptOptions = {
    model: ModelRef | null;
    agent: string;
    variant?: string | null;
    files?: PromptFile[];
    directory?: string;
};
export type PromptSendResult = { ok: true } | { ok: false; error: string };
/** What became of an archived thread due for purging: gone, restored and kept, or not reached this time. */
export type ArchivePurge = "deleted" | "kept" | "failed";
/** `stop` refuses the call and ends the turn; `reject` refuses it and lets the turn go on. */
export type PermissionResponse = "once" | "always" | "reject" | "stop";
export type ProviderAuthResult = { ok: boolean; connected: boolean };
export type SessionMoveResult = { ok: boolean; moved: string[]; error?: string };

const pageSize = 100;
const sessionPageSize = 200;

export function createActions(
    requireClient: () => Client,
    state: EngineState,
    set: SetStoreFunction<EngineState>,
    workspaces: () => WorkspaceIndex,
) {
    const transcriptRequests = new Map<string, Promise<boolean>>();
    const reconciliations = new Map<string, Promise<void>>();
    const reconciliationWanted = new Set<string>();
    /**
     * Submission ids of prompts whose fate is unknown, by session and exact prompt, until the engine answers for sure.
     */
    const unsettled = new Map<string, string>();
    let noticeSequence = 0;

    function notice(input: NoticeInput) {
        pushNotice(set, {
            id: input.id ?? `notice-${Date.now()}-${noticeSequence++}`,
            created: input.created ?? Date.now(),
            duration: input.duration ?? 5000,
            ...input,
        });
    }

    const context: ActionContext = { requireClient, state, set, workspaces, notice };

    function entries(messages: NativeMessageWithParts[]): MessageEntry[] {
        return messages.map(({ parts, ...info }) => ({
            info,
            parts,
        }));
    }

    async function reloadSession(id: string) {
        const captured = captureRevisions(state);
        const existed = id in state.sessions;
        const messages = await requireClient().messages(id, { limit: pageSize });
        const loaded = interruptStaleTools(
            entries(messages).sort(compareMessages),
            state.liveTools,
            t("drift.message.interrupted"),
        );
        if (existed && !state.sessions[id]) return;
        set("transcripts", id, mergeTranscriptSnapshot(state.transcripts[id], loaded, id, captured, state.revisions));
        set("loaded", id, true);
        set("cursors", id, messages.length === pageSize ? messages[0]!.id : null);
        const [todos, tasks] = await Promise.all([
            requireClient()
                .todos(id)
                .catch(() => undefined),
            requireClient()
                .tasks(id)
                .catch(() => undefined),
        ]);
        if (todos) set("todos", id, todos);
        if (tasks) putTasks(set, state, id, tasks);
    }

    // A gap during an existing reload needs a newer snapshot, not that reload's stale response.
    function reconcileSession(id: string) {
        reconciliationWanted.add(id);
        const active = reconciliations.get(id);
        if (active) return active;
        const request = (async () => {
            await transcriptRequests.get(id);
            while (reconciliationWanted.delete(id)) await reloadSession(id);
        })()
            .catch((cause) => {
                notice({
                    id: `transcript-load-${id}`,
                    title: "Transcript load failed",
                    message: errorMessage(cause),
                    variant: "error",
                });
            })
            .finally(() => {
                reconciliations.delete(id);
                reconciliationWanted.delete(id);
            });
        reconciliations.set(id, request);
        return request;
    }

    /** Stops one worker; the engine's `task.updated` reports how it ended. */
    async function stopTask(taskId: string) {
        try {
            const task = await requireClient().stopTask(taskId);
            putTasks(set, state, task.parentSessionId, [task]);
        } catch (cause) {
            notice({
                id: `task-stop-${taskId}`,
                title: t("drift.task.stopFailed"),
                message: errorMessage(cause),
                variant: "error",
            });
        }
    }

    function openSession(id: string) {
        if (state.loaded[id]) return Promise.resolve(true);
        const active = transcriptRequests.get(id);
        if (active) return active;
        let request!: Promise<boolean>;
        request = reloadSession(id)
            .then(() => true)
            .catch((cause) => {
                notice({
                    id: `transcript-load-${id}`,
                    title: "Transcript load failed",
                    message: errorMessage(cause),
                    variant: "error",
                });
                return false;
            })
            .finally(() => {
                if (transcriptRequests.get(id) === request) transcriptRequests.delete(id);
            });
        transcriptRequests.set(id, request);
        return request;
    }

    async function loadOlder(id: string) {
        const cursor = state.cursors[id];
        if (!cursor) return false;
        const older = await requireClient().messages(id, { before: cursor, limit: pageSize });
        const sorted = interruptStaleTools(
            entries(older).sort(compareMessages),
            state.liveTools,
            t("drift.message.interrupted"),
        );
        set(
            produce((draft) => {
                const existing = new Set((draft.transcripts[id] ?? []).map((entry) => entry.info.id));
                draft.transcripts[id] = [
                    ...sorted.filter((entry) => !existing.has(entry.info.id)),
                    ...(draft.transcripts[id] ?? []),
                ];
            }),
        );
        set("cursors", id, older.length === pageSize ? older[0]!.id : null);
        return sorted.length > 0;
    }

    /** Every page of a listing; a truncated snapshot would purge sessions it never saw. */
    async function allPages(params: { workspace?: string; archived?: boolean }) {
        const all: Session[] = [];
        const running: string[] = [];
        let before: string | undefined;
        for (;;) {
            const page = await requireClient().sessions({ ...params, before, limit: sessionPageSize });
            all.push(...page.map((s) => sessionInWorkspace(s, workspaces())));
            running.push(...page.filter((s) => s.running).map((s) => s.id));
            if (page.length < sessionPageSize) return { sessions: all, running };
            before = page[page.length - 1]!.id;
        }
    }

    /** The engine says which sessions have a turn in flight; everything else listed is idle. */
    function reconcileStatus(sessions: Session[], running: Set<string>, captured: Record<string, number>) {
        const statuses = Object.fromEntries(
            sessions.map((s) => [s.id, running.has(s.id) ? { type: "busy" as const } : { type: "idle" as const }]),
        );
        applyStatusSnapshot(set, { sessions, statuses, captured });
    }

    // Loaders snapshot state untracked: effects call them, and they write what they read.
    async function loadSessions(directory: string) {
        const { workspace, captured, epoch } = untrack(() => ({
            workspace: workspaces().id(directory),
            captured: captureRevisions(state),
            epoch: state.sessionSnapshotEpoch,
        }));
        if (!workspace) return;
        const { sessions, running } = await allPages({ workspace });
        if (state.sessionSnapshotEpoch !== epoch) return;
        applySessionSnapshot(set, { sessions, captured, scope: { directory } });
        reconcileStatus(sessions, new Set(running), captured);
    }

    async function loadAllSessions() {
        const captured = untrack(() => captureRevisions(state));
        const [live, archived] = await Promise.all([allPages({}), allPages({ archived: true })]);
        const sessions = [...live.sessions, ...archived.sessions];
        applySessionSnapshot(set, { sessions, captured });
        reconcileStatus(sessions, new Set(live.running), captured);
        set("sessionSnapshotAll", true);
    }

    async function newSession(): Promise<(Session & { discard: () => Promise<void> }) | undefined> {
        const workspaceId = workspaces().id(state.directory);
        if (!workspaceId) return undefined;
        const created = await requireClient().createSession({ workspaceId });
        const session = sessionInWorkspace(created, workspaces());
        // A fresh session is known empty; mark it loaded so the first turn's events are not dropped.
        set(
            produce((draft) => {
                draft.transcripts[session.id] ??= [];
                draft.loaded[session.id] = true;
                draft.cursors[session.id] ??= null;
            }),
        );
        putSession(set, session);
        return { ...session, discard: () => sessions.purgeSession(session.id).then(() => undefined) };
    }

    async function send(id: string, text: string, options: PromptOptions): Promise<PromptSendResult> {
        set("errors", id, undefined!);
        const parts = [
            ...(text.trim() ? [{ type: "text" as const, text }] : []),
            ...(options.files ?? []).map((file) => ({
                type: "file" as const,
                mime: file.mime,
                name: file.filename ?? "file",
                url: file.url,
            })),
        ];
        if (parts.length === 0) return fail(id, "Prompt failed: the prompt is empty");
        // Named only when they change what runs next; an unchanged follow-up steers into the running turn.
        const saved = savedChoice(state, id);
        const prompt = {
            parts,
            model: options.model ? { provider: options.model.providerID, model: options.model.modelID } : undefined,
            ...(options.variant !== undefined && options.variant !== (saved.variant ?? null)
                ? { variant: options.variant }
                : {}),
            ...(options.agent && options.agent !== saved.agent ? { agent: options.agent } : {}),
        };
        // Resending the same prompt reuses its id, so a send whose answer was lost is not admitted twice.
        const key = `${id}\n${JSON.stringify(prompt)}`;
        // Attachments are base64 text, so the request's length in characters is its size in bytes.
        if (key.length > maxRequestBytes)
            return fail(
                id,
                t("drift.prompt.tooLarge", {
                    size: formatAttachmentBytes(key.length),
                    limit: formatAttachmentBytes(maxRequestBytes),
                }),
            );
        const submission = unsettled.get(key) ?? submissionId();
        unsettled.set(key, submission);
        try {
            const receipt = await requireClient().submit(id, { submissionId: submission, ...prompt });
            unsettled.delete(key);
            putSession(set, sessionInWorkspace(receipt.session, workspaces()));
            return { ok: true };
        } catch (cause) {
            if (definite(cause)) unsettled.delete(key);
            return fail(id, `Prompt failed: ${errorMessage(cause)}`);
        }
    }

    function fail(id: string, error: string): PromptSendResult {
        set("errors", id, error);
        return { ok: false, error };
    }

    async function abort(id: string) {
        await requireClient().abort(id);
    }

    /** Paths in the current workspace for an @ mention; the engine ranks them and applies ignore rules. */
    async function findFiles(text: string): Promise<string[]> {
        const workspace = workspaces().id(state.directory);
        return workspace ? requireClient().findFiles(workspace, text) : [];
    }

    async function runCommand(id: string, command: string, args: string) {
        set("errors", id, undefined!);
        try {
            await requireClient().runCommand(id, command, args);
        } catch (cause) {
            fail(id, `Command failed: ${errorMessage(cause)}`);
        }
    }

    const sessions = createSessionActions(context, allPages);

    return {
        ...sessions,
        ...createAskActions(context),
        ...createProviderActions(context),
        ...createConfigActions(context),
        openSession,
        reconcileSession,
        loadOlder,
        loadSessions,
        loadAllSessions,
        newSession,
        send,
        abort,
        stopTask,
        notice,
        findFiles,
        steer: async (id: string, text: string, options: PromptOptions) => send(id, text, options),
        runCommand,
    };
}

export type EngineActions = ReturnType<typeof createActions>;

/** The engine answered and refused: the prompt was not admitted, so a resend may be a new submission. */
function definite(cause: unknown) {
    return cause instanceof EngineError && cause.status >= 400 && cause.status < 500;
}

/** A retried send with the same id gets the original receipt instead of a second turn. */
function submissionId() {
    return typeof crypto !== "undefined" && "randomUUID" in crypto
        ? crypto.randomUUID()
        : `sub_${Date.now()}_${Math.random().toString(36).slice(2)}`;
}
