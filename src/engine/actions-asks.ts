import { EngineError } from "./native/client";
import { questionForCard } from "./questions";
import { produce } from "solid-js/store";

import type { ActionContext } from "./actions-context";
import type { PermissionResponse } from "./actions";

/** Pending permission and question asks: loading them and answering them. */
export function createAskActions({ requireClient, set }: ActionContext) {
    /// Pending asks of both kinds; a single fetch each, applied together so the UI never sees a gap.
    async function refreshPermissions(_directories: string[] = []) {
        const [permissions, questions] = await Promise.all([
            requireClient().permissions(),
            requireClient().questions(),
        ]);
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
            produce(
                (draft) =>
                    void (draft.questions[sessionID] = (draft.questions[sessionID] ?? []).filter(
                        (q) => q.id !== requestID,
                    )),
            ),
        );
    }

    async function replyPermission(sessionID: string, permissionID: string, response: PermissionResponse) {
        try {
            await requireClient().replyPermission(permissionID, { reply: response === "reject" ? "deny" : response });
        } catch (cause) {
            if (cause instanceof EngineError && cause.status === 404) {
                set(
                    produce(
                        (draft) =>
                            void (draft.permissions[sessionID] = (draft.permissions[sessionID] ?? []).filter(
                                (p) => p.id !== permissionID,
                            )),
                    ),
                );
                return;
            }
            throw cause;
        }
    }

    return { refreshPermissions, replyPermission, answerQuestion };
}
