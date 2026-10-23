import { forgetSession, putSession } from "./store";
import { errorMessage } from "./actions-context";
import { sessionInWorkspace } from "./sessions";
import { EngineError } from "./native/client";
import { produce } from "solid-js/store";
import { t } from "../state/i18n";

import type { ArchivePurge, PromptSendResult, SessionMoveResult } from "./actions";
import type { ActionContext } from "./actions-context";
import type { EngineState, ModelRef } from "./store";
import type { components } from "./native/types";
import type { Session } from "./sessions";

type NativeSession = components["schemas"]["Session"];
export type Backup = components["schemas"]["Backup"];
type SessionListing = (params: { workspace?: string }) => Promise<{ sessions: Session[]; running: string[] }>;

/** One session's lifecycle: rename, archive, purge, fork, spawn, compact, undo, retry model and moves. */
export function createSessionActions(context: ActionContext, allPages: SessionListing) {
    const { requireClient, set, workspaces, notice } = context;

    async function rename(id: string, title: string) {
        const updated = await requireClient().updateSession(id, { title });
        putSession(set, sessionInWorkspace(updated, workspaces()));
    }

    /** Archiving stops whatever the session is running, in the engine, before anything else hides it. */
    async function setArchived(id: string, archived: boolean) {
        const updated = await requireClient().updateSession(id, { archived });
        putSession(set, sessionInWorkspace(updated, workspaces()));
    }

    /// Permanent deletion; true only once the engine confirms the row is gone.
    async function purgeSession(id: string) {
        try {
            await requireClient().deleteSession(id);
            set(produce((draft) => purge(draft, id)));
            return true;
        } catch (cause) {
            if (cause instanceof EngineError && cause.status === 404) return true;
            return false;
        }
    }

    /** The archive purge: the engine deletes only what is still archived, so a thread restored meanwhile is `kept`. */
    async function purgeArchivedSession(id: string): Promise<ArchivePurge> {
        try {
            await requireClient().purgeArchivedSession(id);
            set(produce((draft) => purge(draft, id)));
            return "deleted";
        } catch (cause) {
            if (cause instanceof EngineError && cause.status === 404) return "deleted";
            if (cause instanceof EngineError && cause.status === 409) return "kept";
            return "failed";
        }
    }

    /** A removed workspace's purge: true once the engine holds none of its conversations, so its record can go. */
    async function removeAllSessions(workspaceId: string, eligible: () => boolean) {
        if (!eligible()) return false;
        try {
            await requireClient().purgeWorkspace(workspaceId);
            return true;
        } catch (cause) {
            // Unknown to the engine means nothing of it is left; in use or busy waits for the next sweep.
            return cause instanceof EngineError && cause.status === 404;
        }
    }

    /**
     * Copies finished history into a new conversation, through `atMessage` or else everything finished. The copy keeps
     * compaction markers, so it sees the same context.
     */
    async function fork(id: string, atMessage?: string) {
        try {
            const session = sessionInWorkspace(await requireClient().forkSession(id, atMessage), workspaces());
            putSession(set, session);
            return session;
        } catch (cause) {
            notice({
                id: `fork-${id}`,
                title: t("drift.session.forkFailed"),
                message: errorMessage(cause),
                variant: "error",
                duration: 10_000,
            });
        }
    }

    /**
     * `/compact`: the engine summarises now with the compaction agent's model, so the composer's model does not apply.
     */
    async function summarize(id: string, _model?: unknown) {
        try {
            await requireClient().compactSession(id);
        } catch (cause) {
            notice({
                id: `compact-${id}`,
                title: t("drift.session.compactFailed"),
                message: errorMessage(cause),
                variant: "error",
                duration: 10_000,
            });
        }
    }

    /** Undoes back to a prompt, files included unless `keepFiles`; the engine hides it and everything after it. */
    async function revert(id: string, messageID: string, keepFiles = false) {
        return applyUndo(id, () => requireClient().revertSession(id, messageID, keepFiles));
    }

    /** Redoes everything an undo hid, files included. */
    async function unrevert(id: string) {
        return applyUndo(id, () => requireClient().unrevertSession(id));
    }

    async function applyUndo(
        id: string,
        call: () => Promise<{ session: NativeSession; kept: string[]; unattributed: string[]; unrecorded: string[] }>,
    ) {
        try {
            const { session, kept, unattributed, unrecorded } = await call();
            putSession(set, sessionInWorkspace(session, workspaces()));
            // Files the user changed after the session did are never overwritten; say which.
            if (kept.length)
                notice({
                    id: `revert-kept-${id}`,
                    title: t("drift.session.undo.kept"),
                    message: t("drift.session.undo.kept.message", { files: fileSummary(kept) }),
                    variant: "info",
                    duration: 10_000,
                });
            // A command's run shows what changed, not who changed it, so those files are never undone.
            if (unattributed.length)
                notice({
                    id: `revert-unattributed-${id}`,
                    title: t("drift.session.undo.unattributed"),
                    message: t("drift.session.undo.unattributed.message", { files: fileSummary(unattributed) }),
                    variant: "info",
                    duration: 10_000,
                });
            // Imported from opencode without the versions undo needs (older edits, or files changed since).
            if (unrecorded.length)
                notice({
                    id: `revert-unrecorded-${id}`,
                    title: t("drift.session.undo.unrecorded"),
                    message: t("drift.session.undo.unrecorded.message", { files: fileSummary(unrecorded) }),
                    variant: "info",
                    duration: 10_000,
                });
            return true;
        } catch (cause) {
            notice({
                id: `revert-${id}`,
                title: t("drift.session.undoFailed"),
                message: errorMessage(cause),
                variant: "error",
                duration: 10_000,
            });
            return false;
        }
    }

    /**
     * Moves a turn that is waiting to retry onto `model` at `variant` (the model's default when unset); it retries at
     * once.
     */
    async function switchRetryModel(
        id: string,
        _messageID: string,
        model: ModelRef,
        variant?: string,
    ): Promise<PromptSendResult> {
        try {
            await requireClient().switchRetryModel(
                id,
                { provider: model.providerID, model: model.modelID },
                variant ?? null,
            );
            return { ok: true };
        } catch (cause) {
            return { ok: false, error: errorMessage(cause) };
        }
    }

    /** Moves a session with its subagents; the engine refuses while any of them is running. */
    async function moveSession(id: string, destination: string): Promise<SessionMoveResult> {
        const workspaceId = workspaces().id(destination);
        if (!workspaceId) return { ok: false, moved: [], error: t("drift.session.workspaceUnregistered") };
        try {
            return { ok: true, moved: (await requireClient().moveSession(id, workspaceId)).moved };
        } catch (cause) {
            return { ok: false, moved: [], error: errorMessage(cause) };
        }
    }

    /** The sessions and their subagents as one backup file's contents; a failure says why and returns nothing. */
    async function exportSessions(ids: string[]) {
        try {
            return await requireClient().exportSessions(ids);
        } catch (cause) {
            notice({
                id: "backup-export",
                title: t("drift.session.backupFailed"),
                message: errorMessage(cause),
                variant: "error",
            });
        }
    }

    /** Every live thread of the workspace at `directory`, with their subagents, as one backup. */
    async function exportWorkspace(directory: string) {
        const workspace = workspaces().id(directory);
        if (!workspace) return undefined;
        try {
            const { sessions } = await allPages({ workspace });
            return await exportSessions(sessions.map((session) => session.id));
        } catch (cause) {
            notice({
                id: "backup-export",
                title: t("drift.session.backupFailed"),
                message: errorMessage(cause),
                variant: "error",
            });
        }
    }

    /** Restores a backup into the workspace at `directory`; sessions already there are skipped, never overwritten. */
    async function importSessions(directory: string, backup: Backup) {
        const workspaceId = workspaces().id(directory);
        try {
            if (!workspaceId) throw new Error(t("drift.session.workspaceUnregistered"));
            return await requireClient().importSessions(workspaceId, backup);
        } catch (cause) {
            notice({
                id: "backup-import",
                title: t("drift.session.importFailed"),
                message: errorMessage(cause),
                variant: "error",
            });
        }
    }

    /**
     * Sessions belong to the workspace, not its path, so re-pointing a folder moves nothing; a running turn must finish
     * first.
     */
    async function moveWorkspaceSessions(from: string, _to: string): Promise<SessionMoveResult> {
        const workspace = workspaces().id(from);
        if (!workspace) return { ok: true, moved: [] };
        try {
            const { running } = await allPages({ workspace });
            if (running.length)
                return {
                    ok: false,
                    moved: [],
                    error: t("drift.session.moveRunning"),
                };
            return { ok: true, moved: [] };
        } catch (cause) {
            return { ok: false, moved: [], error: errorMessage(cause) };
        }
    }

    /** `/spawn <instruction>`: a new linked thread that starts at once with this conversation and the instruction. */
    async function spawn(id: string, instruction: string) {
        try {
            const session = sessionInWorkspace(await requireClient().spawnThread(id, instruction), workspaces());
            putSession(set, session);
            return session;
        } catch (cause) {
            notice({
                id: `spawn-${id}`,
                title: t("drift.session.spawnFailed"),
                message: errorMessage(cause),
                variant: "error",
                duration: 10_000,
            });
        }
    }

    return {
        rename,
        setArchived,
        purgeSession,
        purgeArchivedSession,
        removeAllSessions,
        fork,
        spawn,
        summarize,
        revert,
        unrevert,
        switchRetryModel,
        moveSession,
        moveWorkspaceSessions,
        exportSessions,
        exportWorkspace,
        importSessions,
    };
}

function purge(draft: EngineState, id: string) {
    forgetSession(draft, id);
    delete draft.transcripts[id];
    delete draft.loaded[id];
    delete draft.permissions[id];
    delete draft.status[id];
    delete draft.running[id];
    delete draft.errors[id];
    delete draft.cursors[id];
    delete draft.tasks[id];
}

/** A notice names a few files and counts the rest, so an undo across many files stays one short line. */
export function fileSummary(paths: string[], shown = 3) {
    const names = paths.slice(0, shown).join(", ");
    return paths.length > shown ? t("drift.session.undo.more", { names, count: paths.length - shown }) : names;
}
