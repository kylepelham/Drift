import { createMemo, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { activateModal, closeOnBackdropPointerDown } from "./modal";
import { ownCompactionPoint } from "../engine/store";
import { pickerWidth } from "./settings-controls";
import { Portal } from "solid-js/web";
import { useEngine } from "../engine";
import { t } from "../state/i18n";
import { Picker } from "./picker";
import { IconX } from "./icons";

import type { EngineState } from "../engine/store";
import type { ModelInfo } from "../engine/catalog";

/** The sizes a model's own compaction point may be set to, below its window's own point. */
const compactSizes = [100_000, 150_000, 200_000, 300_000, 400_000, 600_000, 800_000];
const tokensPerThousand = 1000;
const tokensPerMillion = 1_000_000;
const inheritedId = "inherited";

const noLimit = { context: 0, output: 0 };
const noCost = { input: 0, output: 0, cache_read: 0, cache_write: 0 };

/** Where a model compacts, as the engine resolved it into its catalog entry. */
const compactsAt = (model: ModelInfo) => model.limit?.compact ?? 0;

/** `400k`, `1M`, `1.05M`. */
function formatTokens(tokens: number) {
    if (tokens >= tokensPerMillion) return `${Number((tokens / tokensPerMillion).toFixed(2))}M`;

    return `${Math.round(tokens / tokensPerThousand)}k`;
}

/**
 * Where the newest model of each connected provider compacts, as "Claude Opus 5.5 compacts at 400k", at most two of them;
 * a single provider gives a second model when one compacts elsewhere.
 */
export function compactionExamples(state: Pick<EngineState, "providers" | "connected">) {
    const newest = (models: ModelInfo[]) =>
        [...models]
            .filter((model) => compactsAt(model) > 0 && !model.mode)
            .sort((left, right) => (right.release_date ?? "").localeCompare(left.release_date ?? ""));
    const connected = state.providers.filter((provider) => state.connected.includes(provider.id));
    const picked = connected.flatMap((provider) => newest(Object.values(provider.models)).slice(0, 1));
    if (picked.length === 1) {
        const others = newest(connected.flatMap((provider) => Object.values(provider.models)));
        const elsewhere = others.find((model) => compactsAt(model) !== compactsAt(picked[0]));
        if (elsewhere) picked.push(elsewhere);
    }

    return picked.slice(0, 2).map((model) =>
        t("drift.settings.compactAt.example", {
            model: model.name,
            tokens: formatTokens(compactsAt(model)),
        }),
    );
}

/** A model's resolved limits, abilities and prices, and its own compaction point, over the model manager. */
export function ModelDetails(props: { providerID: string; modelID: string; onClose: () => void }) {
    let dialog!: HTMLDivElement;
    const engine = useEngine();
    const [chosen, setChosen] = createSignal<Record<string, number> | null>(null);
    const [error, setError] = createSignal("");
    const key = () => `${props.providerID}/${props.modelID}`;
    const model = createMemo(
        () => engine.state.providers.find((provider) => provider.id === props.providerID)?.models[props.modelID],
    );
    onMount(() => onCleanup(activateModal(dialog, props.onClose)));
    onMount(
        () =>
            void engine.actions
                .engineSettings()
                .then((settings) => setChosen(settings.modelCompact ?? {}))
                .catch(() => setChosen({})),
    );

    /** Keeps the choice shown and takes it back when the engine refuses it. */
    async function pickCompact(id: string) {
        const before = chosen();
        if (!before) return;
        const next = { ...before };
        if (id === inheritedId) delete next[key()];
        else next[key()] = Number(id);
        setChosen(next);
        setError("");
        try {
            const saved = await engine.actions.putEngineSettings({ modelCompact: next });
            setChosen(saved.modelCompact ?? next);
        } catch (cause) {
            setChosen(before);
            setError(cause instanceof Error ? cause.message : String(cause));
        }
    }

    return (
        <Portal>
            <div
                data-wheel-lock
                data-modal-layer
                class="fixed inset-0 z-40 flex items-center justify-center bg-black/50 p-2 sm:p-4"
                onPointerDown={(event) => closeOnBackdropPointerDown(event, props.onClose, dialog)}
            >
                <div
                    ref={dialog}
                    role="dialog"
                    aria-modal="true"
                    aria-label={model()?.name ?? props.modelID}
                    tabIndex={-1}
                    class="fade-up flex max-h-[calc(100dvh-1rem)] w-[min(30rem,calc(100vw-1rem))] flex-col overflow-hidden rounded-2xl border border-edge bg-overlay shadow-2xl shadow-black/40"
                    onClick={(event) => event.stopPropagation()}
                >
                    <div class="flex items-center justify-between border-b border-edge px-5 py-4">
                        <div class="min-w-0">
                            <div class="truncate text-sm font-semibold text-ink">{model()?.name ?? props.modelID}</div>
                            <div class="truncate text-xs text-ink-faint">{key()}</div>
                        </div>
                        <button
                            title={t("common.close")}
                            class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
                            onClick={() => props.onClose()}
                        >
                            <IconX />
                        </button>
                    </div>
                    <Show when={model()}>
                        {(info) => (
                            <div class="min-h-0 flex-1 overflow-y-auto px-5 py-3">
                                <Facts model={info()} />
                                <CompactRow
                                    model={info()}
                                    chosen={chosen()?.[key()]}
                                    onPick={(id) => void pickCompact(id)}
                                />
                                <Show when={error()}>
                                    <div class="py-2 text-xs text-danger">{error()}</div>
                                </Show>
                                <Prices model={info()} />
                            </div>
                        )}
                    </Show>
                </div>
            </div>
        </Portal>
    );
}

function Fact(props: { label: string; value: string }) {
    return (
        <div class="flex items-center justify-between gap-4 border-b border-edge/70 py-2 text-sm">
            <span class="text-ink-muted">{props.label}</span>
            <span class="text-right text-ink tabular-nums">{props.value}</span>
        </div>
    );
}

function Facts(props: { model: ModelInfo }) {
    const yes = (value: boolean | undefined) => t(value ? "drift.model.details.yes" : "drift.model.details.no");
    const limit = () => props.model.limit ?? noLimit;

    return (
        <>
            <Fact label={t("drift.model.details.window")} value={formatTokens(limit().context)} />
            <Show when={limit().output}>
                <Fact label={t("drift.model.details.output")} value={formatTokens(limit().output)} />
            </Show>
            <Fact label={t("drift.model.details.images")} value={yes(props.model.attachment)} />
            <Fact label={t("drift.model.details.pdfs")} value={yes(props.model.pdf)} />
            <Fact label={t("drift.model.details.reasoning")} value={yes(props.model.reasoning)} />
            <Fact label={t("drift.model.details.toolSearch")} value={yes(props.model.tool_search)} />
        </>
    );
}

/** The model's compaction point: the General setting's, or a size below the window's own point. */
function CompactRow(props: { model: ModelInfo; chosen: number | undefined; onPick: (id: string) => void }) {
    const own = () => ownCompactionPoint(props.model.limit ?? noLimit);
    const items = () => {
        const inherited = props.chosen
            ? t("drift.model.details.compactsAt.inherited")
            : t("drift.model.details.compactsAt.inheritedAt", {
                  tokens: formatTokens(compactsAt(props.model) || own()),
              });
        const sizes = compactSizes.filter((size) => size < own());

        return [
            { id: inheritedId, label: inherited },
            ...sizes.map((size) => ({ id: String(size), label: formatTokens(size) })),
        ];
    };

    return (
        <div class="flex items-center justify-between gap-4 border-b border-edge/70 py-2">
            <div class="min-w-0">
                <div class="text-sm text-ink-muted">{t("drift.model.details.compactsAt")}</div>
                <div class="text-xs text-ink-faint">{t("drift.model.details.compactsAt.description")}</div>
            </div>
            <Show
                when={own() > compactSizes[0]}
                fallback={<span class="text-sm text-ink tabular-nums">{formatTokens(own())}</span>}
            >
                <Picker
                    label={t("drift.model.details.compactsAt")}
                    items={items()}
                    selected={props.chosen ? String(props.chosen) : inheritedId}
                    floating
                    bordered
                    chevronAtEnd
                    placement="below"
                    width={pickerWidth}
                    onPick={props.onPick}
                />
            </Show>
        </div>
    );
}

function Prices(props: { model: ModelInfo }) {
    const cost = () => props.model.cost ?? noCost;
    const free = () => !cost().input && !cost().output;
    const price = (value: number | undefined) => `$${Number((value ?? 0).toFixed(4))}`;
    const line = (tier: { input: number; output: number; cache_read?: number; cache_write?: number }) =>
        t("drift.model.details.price.line", {
            input: price(tier.input),
            output: price(tier.output),
            read: price(tier.cache_read),
            write: price(tier.cache_write),
        });

    return (
        <div class="py-2 text-sm">
            <div class="text-ink-muted">{t("drift.model.details.price")}</div>
            <Show when={!free()} fallback={<div class="text-ink">{t("drift.model.details.free")}</div>}>
                <div class="mt-1 text-xs text-ink tabular-nums">{line(cost())}</div>
                <For each={props.model.cost?.tiers ?? []}>
                    {(tier) => (
                        <div class="mt-1 text-xs text-ink-faint tabular-nums">
                            {t("drift.model.details.price.above", {
                                tokens: formatTokens(tier.above),
                                line: line(tier),
                            })}
                        </div>
                    )}
                </For>
            </Show>
        </div>
    );
}
