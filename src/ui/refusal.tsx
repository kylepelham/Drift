import { modelInfo, savedChoice, type MessageEntry } from "../engine/store";
import { refusalTrigger, type RefusalTrigger } from "../engine/refusal";
import { createMemo, createSignal, For, Show } from "solid-js";
import { toolInfo } from "./tool-labels";
import { useEngine } from "../engine";
import { t } from "../state/i18n";

import type { Message } from "../engine/messages";

const categories = ["cyber", "bio", "frontier_llm", "reasoning_extraction", "general_harms"];
const previewChars = 280;

/** A safety refusal: what the provider said about it, what was new in that request, and a retry the user chooses. */
export function RefusalNotice(props: { entry: MessageEntry; text: string }) {
    const engine = useEngine();
    const info = () => props.entry.info;
    const refusal = () => info().refusal ?? undefined;
    const trigger = createMemo(() => refusalTrigger(engine.state.transcripts[info().sessionId] ?? [], info().id));

    return (
        <div
            class="space-y-2 rounded-lg border border-danger/40 bg-danger/10 px-3 py-2 text-sm text-danger"
            role="alert"
        >
            <div class="break-words">{props.text}</div>
            <Show when={refusal()?.category}>
                {(category) => (
                    <div class="text-xs">{t("drift.refusal.category", { category: categoryLabel(category()) })}</div>
                )}
            </Show>
            <Show when={refusal()?.explanation}>
                {(explanation) => <div class="text-xs break-words text-ink-muted">{explanation()}</div>}
            </Show>
            <Show when={trigger()}>{(found) => <TriggerList trigger={found()} />}</Show>
            <RetryButton info={info()} />
        </div>
    );
}

function categoryLabel(category: string) {
    return categories.includes(category) ? t(`drift.refusal.category.${category}`) : category;
}

function TriggerList(props: { trigger: RefusalTrigger }) {
    return (
        <div class="space-y-1.5 border-t border-danger/25 pt-2 text-xs text-ink-muted">
            <div class="text-ink-faint">{t("drift.refusal.newHeading")}</div>
            <For each={props.trigger.prompts}>
                {(prompt) => (
                    <div class="break-words whitespace-pre-wrap" title={prompt}>
                        <span class="text-ink-faint">{t("drift.refusal.prompt")}: </span>
                        {prompt.length > previewChars ? `${prompt.slice(0, previewChars)}...` : prompt}
                    </div>
                )}
            </For>
            <Show when={props.trigger.files.length}>
                <div class="break-words">{t("drift.refusal.files", { files: props.trigger.files.join(", ") })}</div>
            </Show>
            <Show when={props.trigger.tools.length}>
                <div class="text-ink-faint">{t("drift.refusal.tools")}</div>
                <ul class="space-y-0.5 pl-3">
                    <For each={props.trigger.tools}>
                        {(tool) => {
                            const label = toolInfo(tool);
                            return (
                                <li class="truncate" classList={{ "font-mono": !!label.mono }}>
                                    {label.title ?? tool.name}
                                    <Show when={label.subtitle}> {label.subtitle}</Show>
                                </li>
                            );
                        }}
                    </For>
                </ul>
            </Show>
        </div>
    );
}

/** Offered only when the provider names a model this catalog has; Drift never switches by itself. */
function RetryButton(props: { info: Message }) {
    const engine = useEngine();
    const [sending, setSending] = createSignal(false);
    const target = createMemo(() => {
        const suggested = props.info.refusal?.model;
        const provider = props.info.model?.provider;
        if (!suggested || !provider || suggested === props.info.model?.model) return undefined;

        const ref = { providerID: provider, modelID: suggested };
        const model = modelInfo(engine.state, ref);
        return model ? { ref, name: model.name || suggested } : undefined;
    });

    const retry = async (ref: { providerID: string; modelID: string }) => {
        setSending(true);
        const choice = savedChoice(engine.state, props.info.sessionId);
        await engine.actions.send(props.info.sessionId, t("drift.refusal.retryPrompt"), {
            model: ref,
            agent: choice.agent ?? props.info.agent ?? "build",
        });
        setSending(false);
    };

    return (
        <Show when={target()}>
            {(found) => (
                <button
                    class="rounded-md border border-danger/40 px-2 py-1 text-xs transition-colors hover:bg-danger/15 disabled:opacity-50"
                    disabled={sending()}
                    onClick={() => void retry(found().ref)}
                >
                    {t("drift.refusal.retry", { model: found().name })}
                </button>
            )}
        </Show>
    );
}
