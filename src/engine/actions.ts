import { errorMessage, type ActionContext, type NoticeInput } from "./actions-context";
import { applySessionSnapshot, applyStatusSnapshot, pushNotice } from "./events";
import { connectBrowserView, type BrowserViewHandlers } from "./browser-view";
import { produce, type SetStoreFunction } from "solid-js/store";
import { hiddenParent, sessionInWorkspace } from "./sessions";
import { createProviderActions } from "./actions-providers";
import { createSessionActions } from "./actions-sessions";
import { formatAttachmentBytes } from "../attachments";
import { createConfigActions } from "./actions-config";
import { dropBrowser, putBrowser } from "./browsers";
import { createAskActions } from "./actions-asks";
import { putProcesses } from "./processes";
import { untrack } from "solid-js";
import { t } from "../state/i18n";
import {
    captureRevisions,
    compareMessages,
    interruptStaleTools,
    mergeTranscriptSnapshot,
    normalizeDir,
    putSession,
    putTasks,
    savedChoice,
    sessionBusy,
    type EngineState,
    type MessageEntry,
    type ModelRef,
    type QueuedPrompt,
} from "./store";
import {
    EngineError,
    maxRequestBytes,
    type Client,
    type GitSource,
    type Review,
    type ReviewRefs,
} from "./native/client";

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
    /** Wait for the running turn to end and start a turn of its own, rather than joining the running one. */
    queue?: boolean;
    /** Start the turn in a new worktree; only a thread's first prompt asks for one. */
    worktree?: boolean;
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
            sessionBusy(state, id),
        );

        if (existed && !state.sessions[id]) return;

        set("transcripts", id, mergeTranscriptSnapshot(state.transcripts[id], loaded, id, captured, state.revisions));
        set("loaded", id, true);
        set("cursors", id, messages.length === pageSize ? messages[0]!.id : null);

        const [, todos, tasks, processes, browser] = await Promise.all([
            loadChildren(id).catch(() => undefined),
            requireClient()
                .todos(id)
                .catch(() => undefined),
            requireClient()
                .tasks(id)
                .catch(() => undefined),
            requireClient()
                .processes(id)
                .catch(() => undefined),
            requireClient()
                .browser(id)
                .catch(() => undefined),
        ]);
        if (todos) set("todos", id, todos);
        if (tasks) putTasks(set, state, id, tasks);
        if (processes) putProcesses(set, state, id, processes);
        if (browser) putBrowser(set, browser);
        if (browser === null) dropBrowser(set, id);
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
                    title: t("drift.session.transcriptFailed"),
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

    /** Stops one background process and what it started; `process.updated` reports it ended. */
    async function stopProcess(processId: string) {
        try {
            const record = await requireClient().stopProcess(processId);
            putProcesses(set, state, record.sessionId, [record]);
        } catch (cause) {
            notice({
                id: `process-stop-${processId}`,
                title: t("drift.process.stopFailed"),
                message: errorMessage(cause),
                variant: "error",
            });
        }
    }

    /** Starts a session's browser at the user's request; the pane opens when `browser.updated` reports it. */
    async function startBrowser(sessionId: string) {
        try {
            putBrowser(set, await requireClient().startBrowser(sessionId));
        } catch (cause) {
            notice({
                id: `browser-start-${sessionId}`,
                title: t("drift.browser.startFailed"),
                message: errorMessage(cause),
                variant: "error",
            });
        }
    }

    /** Ends a session's browser; `browser.updated` reports when it has gone. */
    async function stopBrowser(sessionId: string) {
        try {
            await requireClient().stopBrowser(sessionId);
        } catch (cause) {
            notice({
                id: `browser-stop-${sessionId}`,
                title: t("drift.browser.stopFailed"),
                message: errorMessage(cause),
                variant: "error",
            });
        }
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
                    title: t("drift.session.transcriptFailed"),
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
    async function allPages(params: { workspace?: string; archived?: boolean; parent?: string; subagents?: "active" }) {
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

    /** Known subagents a lean listing left out have ended, so they are idle; `directory` narrows it to one workspace. */
    function endedSubagents(listed: Session[], directory?: string) {
        const ids = new Set(listed.map((session) => session.id));
        const scope = directory === undefined ? undefined : normalizeDir(directory);

        return untrack(() =>
            Object.values(state.sessions).filter(
                (session) =>
                    !ids.has(session.id) &&
                    !!hiddenParent(session) &&
                    (scope === undefined || normalizeDir(session.directory) === scope),
            ),
        );
    }

    /** The subagents and threads a session launched: lean listings leave its ended subagents out until it opens. */
    async function loadChildren(id: string) {
        const captured = untrack(() => captureRevisions(state));
        const { sessions, running } = await allPages({ parent: id });
        applySessionSnapshot(set, { sessions, captured });
        reconcileStatus(sessions, new Set(running), captured);
    }

    /** A session selected before it was listed, such as an ended subagent remembered from the last run. */
    async function findSession(id: string) {
        putSession(set, sessionInWorkspace(await requireClient().session(id), workspaces()));
    }

    // Loaders snapshot state untracked: effects call them, and they write what they read.
    async function loadSessions(directory: string) {
        const { workspace, captured, epoch } = untrack(() => ({
            workspace: workspaces().id(directory),
            captured: captureRevisions(state),
            epoch: state.sessionSnapshotEpoch,
        }));
        if (!workspace) return;

        const { sessions, running } = await allPages({ workspace, subagents: "active" });

        if (state.sessionSnapshotEpoch !== epoch) return;

        applySessionSnapshot(set, { sessions, captured, scope: { directory } });
        reconcileStatus([...sessions, ...endedSubagents(sessions, directory)], new Set(running), captured);
    }

    async function loadAllSessions() {
        const captured = untrack(() => captureRevisions(state));
        const [live, archived] = await Promise.all([
            allPages({ subagents: "active" }),
            allPages({ archived: true, subagents: "active" }),
        ]);
        const sessions = [...live.sessions, ...archived.sessions];
        applySessionSnapshot(set, { sessions, captured });
        reconcileStatus([...sessions, ...endedSubagents(sessions)], new Set(live.running), captured);
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

    /** The prompt as the engine takes it, or nothing when it has neither text nor files. */
    function promptBody(id: string, text: string, options: PromptOptions) {
        const parts = [
            ...(text.trim() ? [{ type: "text" as const, text }] : []),
            ...(options.files ?? []).map((file) => ({
                type: "file" as const,
                mime: file.mime,
                name: file.filename ?? "file",
                url: file.url,
            })),
        ];
        if (parts.length === 0) return undefined;

        // Named only when they change what runs next; an unchanged follow-up steers into the running turn.
        const saved = savedChoice(state, id);
        return {
            parts,
            model: options.model ? { provider: options.model.providerID, model: options.model.modelID } : undefined,
            ...(options.variant !== undefined && options.variant !== (saved.variant ?? null)
                ? { variant: options.variant }
                : {}),
            ...(options.agent && options.agent !== saved.agent ? { agent: options.agent } : {}),
            ...(options.worktree ? { worktree: true } : {}),
        };
    }

    /** Queues the prompt behind the running turn, or submits it to steer or start one. */
    async function deliver(id: string, body: Parameters<Client["submit"]>[1], queue: boolean | undefined) {
        if (queue) {
            await requireClient().queuePrompt(id, body);
            return;
        }
        const receipt = await requireClient().submit(id, body);
        putSession(set, sessionInWorkspace(receipt.session, workspaces()));
    }

    async function send(id: string, text: string, options: PromptOptions): Promise<PromptSendResult> {
        set("errors", id, undefined!);

        const prompt = promptBody(id, text, options);
        if (!prompt) return fail(id, t("drift.prompt.failed", { reason: t("drift.prompt.empty") }));
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
            await deliver(id, { submissionId: submission, ...prompt }, options.queue);
            unsettled.delete(key);
            return { ok: true };
        } catch (cause) {
            if (definite(cause)) unsettled.delete(key);
            return fail(id, t("drift.prompt.failed", { reason: errorMessage(cause) }));
        }
    }

    function fail(id: string, error: string): PromptSendResult {
        set("errors", id, error);
        return { ok: false, error };
    }

    async function loadQueue(id: string) {
        set("queued", id, await requireClient().queue(id));
    }

    /** Replaces a queued prompt's text, keeping its files. */
    async function editQueued(queued: QueuedPrompt, text: string) {
        const files = queued.prompt.parts.filter((part) => part.type !== "text");
        const parts = text.trim() ? [{ type: "text" as const, text }, ...files] : files;
        await requireClient().editQueued(queued.id, { ...queued.prompt, parts });
    }

    async function abort(id: string) {
        await requireClient().abort(id);
    }

    /** Paths in the current workspace for an @ mention; the engine ranks them and applies ignore rules. */
    async function findFiles(text: string): Promise<string[]> {
        const workspace = workspaces().id(state.directory);
        return workspace ? requireClient().findFiles(workspace, text) : [];
    }

    /** The current workspace's changes against git, or nothing without a workspace. */
    async function reviewWorkspace(
        source: GitSource,
        target?: string,
        directory?: string,
    ): Promise<Review | undefined> {
        const workspace = workspaces().id(directory ?? state.directory);
        return workspace ? requireClient().reviewWorkspace(workspace, source, target) : undefined;
    }

    async function reviewRefs(directory?: string): Promise<ReviewRefs | undefined> {
        const workspace = workspaces().id(directory ?? state.directory);
        return workspace ? requireClient().reviewRefs(workspace) : undefined;
    }

    async function runCommand(id: string, command: string, args: string) {
        set("errors", id, undefined!);
        try {
            await requireClient().runCommand(id, command, args);
        } catch (cause) {
            fail(id, t("drift.command.failed", { reason: errorMessage(cause) }));
        }
    }

    const sessions = createSessionActions(context, allPages);

    return {
        ...sessions,
        ...createAskActions(context),
        ...createProviderActions(context),
        ...createConfigActions(context),
        openSession,
        findSession,
        reconcileSession,
        loadOlder,
        loadSessions,
        loadAllSessions,
        newSession,
        send,
        loadQueue,
        editQueued,
        discardQueued: (id: string) => requireClient().discardQueued(id),
        sendQueuedNow: (id: string) => requireClient().sendQueuedNow(id),
        abort,
        stopTask,
        stopProcess,
        startBrowser,
        stopBrowser,
        /** Watches a session's browser: frames of its active page and its cursor out, the user's input in. */
        watchBrowser: (sessionId: string, handlers: BrowserViewHandlers) =>
            connectBrowserView(requireClient().browserViewUrl(sessionId), handlers),
        notice,
        findFiles,
        reviewWorkspace,
        reviewRefs,
        reviewTurn: (sessionId: string, messageId: string): Promise<Review> =>
            requireClient().reviewTurn(sessionId, messageId),
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
