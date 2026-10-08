import { activeWorkspace, selectWorkspace, workspaces } from "../state/workspaces";
import { AttentionStrip, PermissionCard, QuestionCard } from "./attention";
import { selectedSession, selectSession } from "../state/selection";
import { createEffect, createSignal, For, Show } from "solid-js";
import { localAsks, resolveAsk } from "../state/asks";
import { normalizeDir } from "../engine/store";
import { useEngine } from "../engine";
import { t } from "../state/i18n";

import type { Permission, QuestionRequest } from "../engine/store";

export function focusedQuestion(questions: QuestionRequest[], requestID?: string) {
    return questions.find((question) => question.id === requestID) ?? questions[0];
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
            <Show when={questions().length > 1}>
                <label class="flex min-w-0 items-center gap-2 px-1 text-xs text-ink-muted">
                    <span class="shrink-0">{t("drift.question.pending", { count: questions().length })}</span>
                    <select
                        class="min-w-0 flex-1 rounded-md border border-edge bg-surface px-2 py-1.5 text-ink"
                        value={pendingQuestion()?.id ?? ""}
                        onChange={(event) => setFocusedQuestionID(event.currentTarget.value)}
                    >
                        <For each={questions()}>
                            {(request) => (
                                <option value={request.id}>
                                    {request.async ? "" : `${t("drift.question.blocking")}: `}
                                    {request.questions[0]?.header || t("drift.question.number", { number: 1 })}
                                    {" - "}
                                    {engine.state.sessions[request.sessionId]?.title ||
                                        t("drift.composer.anotherThread")}
                                </option>
                            )}
                        </For>
                    </select>
                </label>
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
