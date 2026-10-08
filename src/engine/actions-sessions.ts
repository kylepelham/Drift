import { errorMessage } from "./actions-context";
import { sessionInWorkspace } from "./sessions";
import { EngineError } from "./native/client";
import { produce } from "solid-js/store";
import { putSession } from "./store";

import type { ArchivePurge, PromptSendResult, SessionMoveResult } from "./actions";
import type { ActionContext } from "./actions-context";
import type { EngineState, ModelRef } from "./store";
import type { components } from "./native/types";
import type { Session } from "./sessions";

type NativeSession = components["schemas"]["Session"];
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

    /** Copies finished history into a new conversation, through `atMessage` or else everything finished. The copy keeps compaction markers, so it sees the same context. */
    async function fork(id: string, atMessage?: string) {
        try {
            const session = sessionInWorkspace(await requireClient().forkSession(id, atMessage), workspaces());
            putSession(set, session);
            return session;
        } catch (cause) {
            notice({
                id: `fork-${id}`,
                title: "Couldn't fork",
                message: errorMessage(cause),
                variant: "error",
                duration: 10_000,
            });
        }
    }

    /** `/compact`: the engine summarises now with the compaction agent's model, so the composer's model does not apply. */
    async function summarize(id: string, _model?: unknown) {
        try {
            await requireClient().compactSession(id);
        } catch (cause) {
            notice({
                id: `compact-${id}`,
                title: "Couldn't compact",
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
                    title: "Kept your changes",
                    message: `Left as you changed them: ${kept.join(", ")}`,
                    variant: "info",
                    duration: 10_000,
                });
            // A command's run shows what changed, not who changed it, so those files are never undone.
            if (unattributed.length)
                notice({
                    id: `revert-unattributed-${id}`,
                    title: "Left files changed during commands",
                    message: `Changed while a command ran, so not undone: ${unattributed.join(", ")}`,
                    variant: "info",
                    duration: 10_000,
                });
            // Imported from opencode without the versions undo needs (older edits, or files changed since).
            if (unrecorded.length)
                notice({
                    id: `revert-unrecorded-${id}`,
                    title: "Some imported edits were not undone",
                    message: `No undo record, left as they are: ${unrecorded.join(", ")}`,
                    variant: "info",
                    duration: 10_000,
                });
            return true;
        } catch (cause) {
            notice({
                id: `revert-${id}`,
                title: "Couldn't undo",
                message: errorMessage(cause),
                variant: "error",
                duration: 10_000,
            });
            return false;
        }
    }

    /** Moves a turn that is waiting to retry onto `model` at `variant` (the model's default when unset); it retries at once. */
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
        if (!workspaceId) return { ok: false, moved: [], error: "That workspace is not registered with the engine" };
        try {
            return { ok: true, moved: (await requireClient().moveSession(id, workspaceId)).moved };
        } catch (cause) {
            return { ok: false, moved: [], error: errorMessage(cause) };
        }
    }

    /** Sessions belong to the workspace, not its path, so re-pointing a folder moves nothing; a running turn must finish first. */
    async function moveWorkspaceSessions(from: string, _to: string): Promise<SessionMoveResult> {
        const workspace = workspaces().id(from);
        if (!workspace) return { ok: true, moved: [] };
        try {
            const { running } = await allPages({ workspace });
            if (running.length)
                return {
                    ok: false,
                    moved: [],
                    error: "Stop the running threads in this workspace first; they keep the folder they started in.",
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
                title: "Couldn't spawn the thread",
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
    };
}

function purge(draft: EngineState, id: string) {
    delete draft.sessions[id];
    delete draft.transcripts[id];
    delete draft.loaded[id];
    delete draft.permissions[id];
    delete draft.status[id];
    delete draft.errors[id];
    delete draft.cursors[id];
    delete draft.tasks[id];
}
