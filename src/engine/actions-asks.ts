import { asksRevisionKey, bumpRevision } from "./store";
import { EngineError } from "./native/client";
import { questionForCard } from "./questions";
import { produce } from "solid-js/store";

import type { ActionContext } from "./actions-context";
import type { PermissionResponse } from "./actions";
import type { Client } from "./native/client";

/** Pending permission and question asks: loading them and answering them. */
export function createAskActions({ requireClient, state, set }: ActionContext) {
    let polls = 0;

    /**
     * Pending asks of both kinds; a single fetch each, applied together so the UI never sees a gap. A poll
     * overtaken by a newer one is dropped, and one an ask changed under is asked again, so neither lands stale.
     */
    async function refreshPermissions(_directories: string[] = []) {
        const mine = ++polls;
        for (let attempt = 0; attempt < 3; attempt += 1) {
            const before = state.revisions[asksRevisionKey] ?? 0;
            const [permissions, questions] = await Promise.all([
                requireClient().permissions(),
                requireClient().questions(),
            ]);
            if (mine !== polls) return;
            if ((state.revisions[asksRevisionKey] ?? 0) !== before) continue;
            return showAsks(permissions, questions);
        }
    }

    function showAsks(
        permissions: Awaited<ReturnType<Client["permissions"]>>,
        questions: Awaited<ReturnType<Client["questions"]>>,
    ) {
        set(
            produce((draft) => {
                draft.permissions = {};
                draft.questions = {};
                for (const request of permissions) {
                    const directory = draft.sessions[request.sessionId]?.directory ?? "";
                    const permission = { ...request, directory };
                    (draft.permissions[request.sessionId] ??= []).push(permission);
                }
                for (const request of questions)
                    (draft.questions[request.sessionId] ??= []).push(questionForCard(request));
                bumpRevision(draft, asksRevisionKey);
            }),
        );
    }

    async function answerQuestion(sessionID: string, requestID: string, answers: string[][] | null) {
        try {
            if (answers) await requireClient().answerQuestion(requestID, answers);
            else await requireClient().rejectQuestion(requestID);
        } catch (cause) {
            if (!(cause instanceof EngineError && cause.status === 404)) throw cause;
        }
        set(
            produce((draft) => {
                draft.questions[sessionID] = (draft.questions[sessionID] ?? []).filter((q) => q.id !== requestID);
                bumpRevision(draft, asksRevisionKey);
            }),
        );
    }

    async function replyPermission(sessionID: string, permissionID: string, response: PermissionResponse) {
        try {
            await requireClient().replyPermission(permissionID, { reply: response === "reject" ? "deny" : response });
        } catch (cause) {
            if (cause instanceof EngineError && cause.status === 404) {
                set(
                    produce((draft) => {
                        draft.permissions[sessionID] = (draft.permissions[sessionID] ?? []).filter(
                            (p) => p.id !== permissionID,
                        );
                        bumpRevision(draft, asksRevisionKey);
                    }),
                );
                return;
            }
            throw cause;
        }
    }

    return { refreshPermissions, replyPermission, answerQuestion };
}
