import { createSignal, For, onMount, Show } from "solid-js";
import { formatModelContext } from "../state/lm-studio";
import { useEngine } from "../engine";
import { t } from "../state/i18n";
import { Picker } from "./picker";

import type { LocalServer } from "../engine/native/client";
import type { ProviderNotice } from "./settings-providers";

/** Windows offered as a limit, in tokens; 0 runs each model at the full window it was trained for. */
const limits = [0, 8_192, 16_384, 32_768, 65_536, 131_072, 262_144];

/** Local Ollama: where it listens, what it has installed, and the largest window its models run with. */
export function OllamaConnect(props: { onNotice: (notice: ProviderNotice) => void }) {
    const engine = useEngine();
    const [server, setServer] = createSignal<LocalServer>();
    const [address, setAddress] = createSignal("");
    const [saving, setSaving] = createSignal(false);
    const connected = () => engine.state.connected.includes("ollama");
    const models = () => Object.values(engine.state.providers.find((item) => item.id === "ollama")?.models ?? {});
    const limit = () => server()?.contextLimit ?? 0;

    // A limit set by hand in drift.json is offered too, so the picker can show it.
    const limitItems = () => {
        const offered = limits.includes(limit()) ? limits : [...limits, limit()].sort((a, b) => a - b);

        return offered.map((tokens) => ({
            id: String(tokens),
            label: tokens === 0 ? t("drift.ollama.limit.full") : formatModelContext(tokens),
        }));
    };

    function show(loaded: LocalServer) {
        setServer(loaded);
        setAddress(loaded.address === loaded.defaultAddress ? "" : loaded.address);
    }

    onMount(() => {
        void engine.actions
            .localServer("ollama")
            .then(show)
            .catch(() => {});
    });

    async function save(contextLimit: number) {
        setSaving(true);
        try {
            const saved = await engine.actions.setLocalServer("ollama", {
                address: address().trim() || null,
                contextLimit,
            });
            show(saved);
            props.onNotice(
                connected()
                    ? { tone: "success", text: t("drift.ollama.saved", { count: models().length }) }
                    : { tone: "warning", text: t("drift.ollama.unavailable", { address: saved.address }) },
            );
        } catch (cause) {
            const reason = cause instanceof Error ? cause.message : String(cause);
            props.onNotice({ tone: "error", text: t("drift.ollama.saveFailed", { reason }) });
        } finally {
            setSaving(false);
        }
    }

    return (
        <div class="mx-3 mb-3 space-y-3 rounded-lg border border-edge bg-surface/55 p-3 shadow-sm shadow-black/5">
            <p class="text-xs text-ink-muted">{t("drift.ollama.description")}</p>
            <Show
                when={connected()}
                fallback={
                    <div class="rounded-md border border-warn/30 bg-warn/10 px-2.5 py-2 text-xs text-warn">
                        {t("drift.ollama.unavailable", { address: server()?.address ?? "" })}
                    </div>
                }
            >
                <div class="text-xs text-ink-muted">{t("drift.ollama.found", { count: models().length })}</div>
                <div class="max-h-52 space-y-1 overflow-y-auto">
                    <For each={models()}>
                        {(model) => (
                            <div class="flex items-center gap-2 rounded-md border border-edge bg-overlay/35 px-2.5 py-2">
                                <span class="min-w-0 flex-1 truncate text-xs text-ink">{model.name}</span>
                                <span class="shrink-0 text-[0.65rem] tabular-nums text-ink-faint">
                                    {model.limit?.context
                                        ? formatModelContext(model.limit.context)
                                        : t("drift.ollama.windowUnknown")}
                                </span>
                            </div>
                        )}
                    </For>
                </div>
            </Show>
            <div class="space-y-1">
                <span class="block text-xs text-ink">{t("drift.ollama.address")}</span>
                <div class="flex flex-wrap gap-2">
                    <input
                        class="h-9 min-w-44 flex-1 rounded-md border border-edge bg-overlay/50 px-2.5 text-sm outline-none transition-colors focus:border-edge-strong"
                        placeholder={server()?.defaultAddress ?? ""}
                        aria-label={t("drift.ollama.address")}
                        value={address()}
                        onInput={(event) => setAddress(event.currentTarget.value)}
                        onKeyDown={(event) => event.key === "Enter" && void save(limit())}
                    />
                    <button
                        class="h-9 rounded-md bg-accent px-3.5 text-xs font-medium text-accent-ink transition-colors hover:brightness-105 disabled:opacity-40"
                        disabled={saving() || !server()}
                        onClick={() => void save(limit())}
                    >
                        {saving() ? t("common.loading") : t("common.save")}
                    </button>
                </div>
            </div>
            <div class="flex items-center gap-3">
                <span class="min-w-0 flex-1">
                    <span class="block text-xs text-ink">{t("drift.ollama.limit.title")}</span>
                    <span class="block text-[0.68rem] text-ink-faint">{t("drift.ollama.limit.description")}</span>
                </span>
                <Picker
                    label={t("drift.ollama.limit.title")}
                    items={limitItems()}
                    selected={String(limit())}
                    floating
                    bordered
                    chevronAtEnd
                    placement="below"
                    width="9rem"
                    onPick={(value) => void save(Number(value))}
                />
            </div>
        </div>
    );
}
