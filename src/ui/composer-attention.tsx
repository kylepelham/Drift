import { activeWorkspace, selectWorkspace, workspaces } from "../state/workspaces";
import { AttentionStrip, PermissionCard, QuestionCard } from "./attention";
import { selectedSession, selectSession } from "../state/selection";
import { createEffect, createSignal, Show } from "solid-js";
import { localAsks, resolveAsk } from "../state/asks";
import { normalizeDir } from "../engine/store";
import { useEngine } from "../engine";
import { t } from "../state/i18n";

import type { RequestStack, RequestStackItem } from "./request-stack";
import type { Permission, QuestionRequest } from "../engine/store";

export function focusedQuestion(questions: QuestionRequest[], requestID?: string) {
    return questions.find((question) => question.id === requestID) ?? questions[0];
}

/** Waiting questions as the card's stack lists them: what each asks and which thread it came from. */
export function questionStackItems(
    questions: QuestionRequest[],
    threadTitle: (sessionId: string) => string | undefined,
): RequestStackItem[] {
    return questions.map((request) => ({
        id: request.id,
        title: request.questions[0]?.header || t("drift.question.number", { number: 1 }),
        thread: threadTitle(request.sessionId) || t("drift.composer.anotherThread"),
        blocking: !request.async,
    }));
}

export function selectOwningSession(
    sessionID: string,
    directory: string | undefined,
    availableWorkspaces: { id: string; path: string }[],
    activeWorkspaceID: string | undefined,
    chooseWorkspace: (id: string) => void,
    chooseSession: (id: string) => void,
) {
    const workspace = directory
        ? availableWorkspaces.find((item) => normalizeDir(item.path) === normalizeDir(directory))
        : undefined;
    if (workspace && workspace.id !== activeWorkspaceID) chooseWorkspace(workspace.id);
    chooseSession(sessionID);
}

/** Permission, question and ask cards stacked above the composer, from every thread. */
export function ComposerAttention() {
    const engine = useEngine();
    const [focusedQuestionID, setFocusedQuestionID] = createSignal<string>();

    const permissions = () => Object.values(engine.state.permissions).flat();
    const questions = () => Object.values(engine.state.questions).flat();
    const pendingPermission = (): Permission | undefined => permissions()[0];
    const pendingQuestion = () => focusedQuestion(questions(), focusedQuestionID());
    const pendingAsk = () => localAsks()[0];

    // Every waiting question rides in the front card's stack, which brings another one forward.
    const questionStack = (): RequestStack => ({
        items: questionStackItems(questions(), (sessionId) => engine.state.sessions[sessionId]?.title),
        current: pendingQuestion()?.id ?? "",
        onSelect: setFocusedQuestionID,
    });

    createEffect(() => {
        const next = pendingQuestion()?.id;
        if (next !== focusedQuestionID()) setFocusedQuestionID(next);
    });

    function openAttentionSession(sessionID: string, directory?: string) {
        selectOwningSession(
            sessionID,
            directory ?? engine.state.sessions[sessionID]?.directory,
            workspaces(),
            activeWorkspace()?.id,
            selectWorkspace,
            selectSession,
        );
    }

    return (
        <div class="composer-attention-stack mx-auto flex w-full max-w-3xl flex-col gap-2">
            <AttentionStrip />
            <Show when={pendingPermission()}>
                {(permission) => (
                    <div class="flow-root">
                        <PermissionCard
                            permission={permission()}
                            thread={
                                permission().sessionId !== selectedSession()
                                    ? {
                                          label: t("drift.composer.pendingInThread", {
                                              thread:
                                                  engine.state.sessions[permission().sessionId]?.title ||
                                                  t("drift.composer.anotherThread"),
                                          }),
                                          onOpen: () =>
                                              openAttentionSession(permission().sessionId, permission().directory),
                                      }
                                    : undefined
                            }
                        />
                    </div>
                )}
            </Show>
            <Show keyed when={pendingQuestion()?.id}>
                {(questionID) => {
                    const question = () => questions().find((item) => item.id === questionID);
                    return (
                        <Show when={question()}>
                            {(request) => (
                                <div class="flow-root">
                                    <QuestionCard
                                        requestID={questionID}
                                        async={request().async}
                                        questions={[...request().questions]}
                                        stack={questionStack()}
                                        thread={
                                            request().sessionId !== selectedSession()
                                                ? {
                                                      label: t("drift.composer.pendingInThread", {
                                                          thread:
                                                              engine.state.sessions[request().sessionId]?.title ||
                                                              t("drift.composer.anotherThread"),
                                                      }),
                                                      onOpen: () =>
                                                          openAttentionSession(
                                                              request().sessionId,
                                                              request().directory,
                                                          ),
                                                  }
                                                : undefined
                                        }
                                        onAnswer={(answers) =>
                                            engine.actions.answerQuestion(request().sessionId, questionID, answers)
                                        }
                                    />
                                </div>
                            )}
                        </Show>
                    );
                }}
            </Show>
            <Show when={pendingAsk()}>
                {(ask) => (
                    <div class="flow-root">
                        <QuestionCard
                            requestID={ask().id}
                            questions={ask().questions}
                            onAnswer={(answers) => {
                                resolveAsk(ask().id, answers);
                                return true;
                            }}
                        />
                    </div>
                )}
            </Show>
        </div>
    );
}
