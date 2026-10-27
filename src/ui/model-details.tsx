import { createMemo, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { activateModal, closeOnBackdropPointerDown } from "./modal";
import { ownCompactionPoint } from "../engine/store";
import { pickerWidth } from "./settings-controls";
import { IconCheck, IconX } from "./icons";
import { Portal } from "solid-js/web";
import { useEngine } from "../engine";
import { t } from "../state/i18n";
import { Picker } from "./picker";

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
                    class="fade-up flex max-h-[calc(100dvh-1rem)] w-[min(32rem,calc(100vw-1rem))] flex-col overflow-hidden rounded-2xl border border-edge bg-overlay shadow-2xl shadow-black/40"
                    onClick={(event) => event.stopPropagation()}
                >
                    <div class="flex items-start justify-between gap-4 px-6 pt-6 pb-4">
                        <div class="min-w-0">
                            <div class="truncate text-lg font-semibold text-ink">{model()?.name ?? props.modelID}</div>
                            <div class="mt-1 text-xs text-ink-faint">
                                {engine.state.providers.find((provider) => provider.id === props.providerID)?.name}
                            </div>
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
                            <div class="min-h-0 flex-1 space-y-5 overflow-y-auto px-6 pb-6">
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

function Limit(props: { label: string; value: number }) {
    return (
        <div class="min-w-0">
            <div class="text-2xl font-medium text-ink tabular-nums">{formatTokens(props.value)}</div>
            <div class="mt-1 text-xs text-ink-faint">{props.label}</div>
        </div>
    );
}

function Facts(props: { model: ModelInfo }) {
    const limit = () => props.model.limit ?? noLimit;

    return (
        <>
            <div class="flex flex-wrap gap-2">
                <Capability label={t("drift.model.details.images")} supported={props.model.attachment} />
                <Capability label={t("drift.model.details.pdfs")} supported={props.model.pdf} />
                <Capability label={t("drift.model.details.reasoning")} supported={props.model.reasoning} />
                <Capability label={t("drift.model.details.toolSearch")} supported={props.model.tool_search} />
            </div>
            <div class="grid grid-cols-2 gap-4 rounded-xl bg-surface/60 px-4 py-4">
                <Limit label={t("drift.model.details.window")} value={limit().context} />
                <Show when={limit().output}>
                    <Limit label={t("drift.model.details.output")} value={limit().output} />
                </Show>
            </div>
        </>
    );
}

function Capability(props: { label: string; supported?: boolean }) {
    const status = () => t(props.supported ? "drift.model.details.supported" : "drift.model.details.unsupported");

    return (
        <span
            aria-label={`${props.label}: ${status()}`}
            title={status()}
            class="inline-flex items-center gap-1.5 rounded-md border border-edge/70 px-2 py-1 text-xs"
            classList={{ "text-ink-muted": !!props.supported, "text-ink-faint": !props.supported }}
        >
            <Show when={props.supported} fallback={<IconX class="size-3" />}>
                <IconCheck class="size-3" />
            </Show>
            {props.label}
        </span>
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
        <div class="flex flex-wrap items-center justify-between gap-3 border-y border-edge/70 py-4">
            <div class="min-w-0">
                <div class="text-sm font-medium text-ink">{t("drift.model.details.compactsAt")}</div>
                <div class="mt-1 text-xs text-ink-faint">{t("drift.model.details.compactsAt.description")}</div>
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

    return (
        <section>
            <h3 class="text-sm font-medium text-ink">{t("drift.model.details.price")}</h3>
            <Show
                when={!free()}
                fallback={<div class="mt-2 text-sm text-ink-muted">{t("drift.model.details.free")}</div>}
            >
                <div class="mt-3 overflow-x-auto">
                    <table class="w-full min-w-[24rem] text-right text-xs tabular-nums">
                        <thead>
                            <tr class="border-b border-edge/70 text-ink-faint">
                                <th scope="col" class="pr-3 pb-2 text-left font-normal">
                                    {t("drift.model.details.price.context")}
                                </th>
                                <For each={["input", "output", "read", "write"] as const}>
                                    {(column) => (
                                        <th scope="col" class="pb-2 pl-3 font-normal">
                                            {t(`drift.model.details.price.${column}`)}
                                        </th>
                                    )}
                                </For>
                            </tr>
                        </thead>
                        <tbody class="text-ink-muted">
                            <PriceRow label={t("drift.model.details.price.base")} cost={cost()} />
                            <For each={props.model.cost?.tiers ?? []}>
                                {(tier) => (
                                    <PriceRow
                                        label={t("drift.model.details.price.above", {
                                            tokens: formatTokens(tier.above),
                                        })}
                                        cost={tier}
                                    />
                                )}
                            </For>
                        </tbody>
                    </table>
                </div>
            </Show>
        </section>
    );
}

function PriceRow(props: { label: string; cost: NonNullable<ModelInfo["cost"]> }) {
    const values = () => [props.cost.input, props.cost.output, props.cost.cache_read, props.cost.cache_write];
    const price = (value: number) => `$${Number(value.toFixed(4))}`;

    return (
        <tr class="border-b border-edge/40 last:border-0">
            <th scope="row" class="py-2.5 pr-3 text-left font-normal text-ink-faint">
                {props.label}
            </th>
            <For each={values()}>{(value) => <td class="py-2.5 pl-3">{price(value)}</td>}</For>
        </tr>
    );
}
