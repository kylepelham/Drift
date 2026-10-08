import { collapseCompaction, orderedModelProviderIds, prefsFor, updatePrefs } from "../state/prefs";
import { createMemo, createSignal, onCleanup, Show, untrack } from "solid-js";
import { compactionThinkingRow, timelinePitch } from "./timeline-state";
import { lmStudioModelReady } from "../state/lm-studio";
import { Picker, type PickerItem } from "./picker";
import { messageModel } from "../engine/messages";
import { variantNames } from "../engine/catalog";
import { ProviderIcon } from "./provider-icon";
import { TextShimmer } from "./text-shimmer";
import { MessageView } from "./message";
import { useEngine } from "../engine";
import { Chevron } from "./controls";
import { t } from "../state/i18n";
import {
    modelInfo,
    savedChoice,
    type EngineState,
    type MessageEntry,
    type ModelRef,
    type SessionStatus,
} from "../engine/store";

import type { PartGroup } from "./message-groups";

// Messages younger than this are treated as newly arrived rather than restored history.
const freshMessageMs = 2000;
const maxRetryMessageChars = 80;

export function Row(props: {
    entry: MessageEntry;
    next?: MessageEntry;
    nextThinking: boolean;
    groups?: PartGroup[];
    thinking: boolean;
    thinkingCompaction?: boolean;
    thinkingHeading?: string;
    retry?: Extract<SessionStatus, { type: "retry" }>;
    terminalError: boolean;
    found: boolean;
    measure: (element: HTMLDivElement) => void;
    copy?: { id: string; source: string; shown: boolean };
    copied: boolean;
    instruction: boolean;
    toggleCopy: (id: string) => void;
}) {
    // Assistant rows remount during virtualization and session switches; replaying an entrance
    // animation on those makes streamed output flicker, so only fresh user rows fade in.
    const fadeIn = untrack(
        () => Date.now() - props.entry.info.createdAt < freshMessageMs && props.entry.info.role === "user",
    );
    const pitch = () => {
        if (props.nextThinking) return "none";
        if (props.next) return timelinePitch(props.entry, props.next);

        return props.terminalError ? "turn" : "none";
    };
    // A running compaction animates its own divider label, so the generic indicator would double up.
    const compactionShimmer = () =>
        props.thinking && !!props.thinkingCompaction && compactionThinkingRow(props.entry, collapseCompaction());
    return (
        <div
            ref={props.measure}
            data-mid={props.entry.info.id}
            class="min-w-0 max-w-full"
            classList={{
                "fade-up": fadeIn,
                "pb-3": pitch() === "part",
                "pb-6": pitch() === "turn",
                "search-hit": props.found,
            }}
        >
            <Show when={props.copy}>
                {(copy) => (
                    <button
                        type="button"
                        class="mb-4 flex w-full items-center gap-3 py-1 text-xs text-ink-faint transition-colors select-none hover:text-ink-muted"
                        aria-expanded={copy().shown}
                        onClick={() => props.toggleCopy(copy().id)}
                    >
                        <div class="h-px flex-1 bg-edge" />
                        <span class="flex min-w-0 items-center gap-1.5">
                            <Chevron open={copy().shown} />
                            <span class="truncate">{t("drift.chat.spawned.copy", { title: copy().source })}</span>
                        </span>
                        <div class="h-px flex-1 bg-edge" />
                    </button>
                )}
            </Show>
            <div classList={{ "border-l-2 border-edge pl-3": props.copied }}>
                <MessageView
                    entry={props.entry}
                    hideError={!!props.retry || !!props.next}
                    footer={props.next?.info.role !== "assistant"}
                    groups={props.groups}
                    thinking={compactionShimmer()}
                    spawned={props.instruction}
                />
            </div>
            <Show when={props.thinking && !compactionShimmer()}>
                <div class="timeline-thinking select-none" role="status" aria-live="polite">
                    <TextShimmer text={t("drift.chat.thinking")} />
                    <Show when={props.thinkingHeading}>
                        {(heading) => <span class="timeline-thinking-heading">{heading()}</span>}
                    </Show>
                </div>
            </Show>
            <Show when={props.retry}>
                {(status) => (
                    <SessionRetry
                        status={status()}
                        sessionID={props.entry.info.sessionId}
                        messageID={props.entry.info.id}
                        model={props.entry.info.role === "assistant" ? messageModel(props.entry.info) : undefined}
                    />
                )}
            </Show>
        </div>
    );
}

function SessionRetry(props: {
    status: Extract<SessionStatus, { type: "retry" }>;
    sessionID: string;
    messageID: string;
    model?: ModelRef;
}) {
    const engine = useEngine();
    const [now, setNow] = createSignal(Date.now());
    // Message updates carrying the pre-switch model would snap a plain mirror of props back to the
    // old selection; a local accepted choice wins until the engine converges on it.
    const [chosen, setChosen] = createSignal<ModelRef>();
    const [submitting, setSubmitting] = createSignal(false);
    const timer = setInterval(() => setNow(Date.now()), 1000);
    onCleanup(() => clearInterval(timer));
    const selected = () => chosen() ?? props.model;
    const display = createMemo(() => retryPresentation(props.status, now()));
    const items = createMemo(() => retryModelItems(engine.state));
    const selectedID = () => {
        const model = selected();
        return model ? `${model.providerID}/${model.modelID}` : undefined;
    };

    async function switchModel(id: string) {
        if (submitting()) return;
        const [providerID, ...rest] = id.split("/");
        const model = { providerID, modelID: rest.join("/") };
        const preferredVariant = prefsFor(props.sessionID, savedChoice(engine.state, props.sessionID)).variant;
        const variants = variantNames(modelInfo(engine.state, model));
        const variant = preferredVariant && variants.includes(preferredVariant) ? preferredVariant : undefined;
        setSubmitting(true);
        const result = await engine.actions.switchRetryModel(props.sessionID, props.messageID, model, variant);
        setSubmitting(false);
        if (!result.ok) {
            engine.actions.notice({
                message: result.error,
                variant: "error",
            });
            return;
        }
        setChosen(model);
        updatePrefs(props.sessionID, { model, variant: variant ?? null });
    }

    return (
        <div
            class="mt-3 rounded-lg border border-danger/40 bg-danger/10 px-3 py-2 text-sm text-danger"
            role="status"
            aria-live="polite"
        >
            <div class="flex flex-wrap items-start justify-between gap-3">
                <div class="flex min-w-0 flex-1 items-start gap-2">
                    <span class="pulse-soft mt-1.5 size-2 shrink-0 rounded-full bg-danger" aria-hidden="true" />
                    <div class="min-w-0">
                        <div
                            class="break-words"
                            classList={{ "cursor-help": display().truncated }}
                            title={display().truncated ? props.status.message : undefined}
                        >
                            {display().message}
                        </div>
                        <div class="mt-0.5 text-xs text-danger/75">{display().info}</div>
                    </div>
                </div>
                <Show when={props.model}>
                    <Picker
                        label={submitting() ? t("drift.chat.retry.switchingModel") : t("drift.chat.retry.switchModel")}
                        items={items()}
                        selected={selectedID()}
                        fallbackLabel={selectedID()}
                        icon={<ProviderIcon id={selected()?.providerID} class="size-3.5 shrink-0" />}
                        bordered
                        floating
                        placement="above"
                        onPick={(id) => void switchModel(id)}
                    />
                </Show>
            </div>
        </div>
    );
}

export function retryModelItems(state: EngineState): PickerItem[] {
    const providers = state.providers.filter((provider) => {
        if (provider.id === "lmstudio") return state.connected.includes(provider.id);
        // Before the first listing lands there is nothing to filter against, so every provider shows.
        // Once the engine is online an empty list is the answer, not a gap: retrying on a disconnected
        // provider only fails again.
        return state.connected.includes(provider.id) || (state.connection !== "online" && state.connected.length === 0);
    });
    return orderedModelProviderIds(providers.map((provider) => provider.id)).flatMap((providerID) => {
        const provider = providers.find((item) => item.id === providerID);
        if (!provider) return [];
        return Object.values(provider.models)
            .filter((model) => provider.id !== "lmstudio" || lmStudioModelReady(model))
            .sort((a, b) => a.name.localeCompare(b.name))
            .map((model) => ({
                id: `${provider.id}/${model.id}`,
                label: model.name,
                group: provider.name,
                providerID: provider.id,
                family: model.family,
                releaseDate: model.release_date,
            }));
    });
}

export function retryPresentation(status: Extract<SessionStatus, { type: "retry" }>, now: number) {
    const normalized = status.message.trim() || t("drift.chat.retry.providerRejected");
    const truncated = normalized.length > maxRetryMessageChars;
    const message = truncated ? normalized.slice(0, maxRetryMessageChars) + "..." : normalized;
    const seconds = Math.max(0, Math.round((status.next - now) / 1000));
    const retry = seconds > 0 ? t("drift.chat.retry.inSeconds", { seconds }) : t("drift.chat.retry.now");
    return {
        message,
        truncated,
        info: t("drift.chat.retry.info", { retry, attempt: status.attempt }),
    };
}
