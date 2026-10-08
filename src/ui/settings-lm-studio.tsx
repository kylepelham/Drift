import { formatModelContext, lmStudioMinimumContext, lmStudioModelReady } from "../state/lm-studio";
import { createSignal, For, Show } from "solid-js";
import { useEngine } from "../engine";
import { t } from "../state/i18n";

import type { ProviderNotice } from "./settings-providers";

export function LmStudioConnect(props: { providerName: string; onNotice: (notice: ProviderNotice) => void }) {
    const engine = useEngine();
    const [pending, setPending] = createSignal<"refresh" | "token" | null>(null);
    const [key, setKey] = createSignal("");
    const connected = () => engine.state.connected.includes("lmstudio");
    const models = () => Object.values(engine.state.providers.find((item) => item.id === "lmstudio")?.models ?? {});
    const ready = () => models().filter(lmStudioModelReady);

    async function refresh() {
        setPending("refresh");
        const ok = await engine.actions.reloadProviders();
        setPending(null);
        if (!ok) {
            props.onNotice({ tone: "error", text: t("drift.lmStudio.refreshFailed") });
            return;
        }

        props.onNotice({
            tone: connected() ? "success" : "warning",
            text: connected()
                ? t("drift.lmStudio.refreshed", { count: ready().length })
                : t("drift.lmStudio.unavailable"),
        });
    }

    async function saveToken() {
        if (!key().trim()) return;

        setPending("token");
        const result = await engine.actions.setProviderKey("lmstudio", key().trim());
        setPending(null);
        if (!result.ok) {
            props.onNotice({
                tone: "error",
                text: t("drift.provider.connectFailed", { provider: props.providerName }),
            });
            return;
        }

        setKey("");
        props.onNotice({
            tone: result.connected ? "success" : "warning",
            text: result.connected
                ? t("drift.lmStudio.refreshed", { count: ready().length })
                : t("drift.provider.savedUnavailable", { provider: props.providerName }),
        });
    }

    return (
        <div class="mx-3 mb-3 space-y-3 rounded-lg border border-edge bg-surface/55 p-3 shadow-sm shadow-black/5">
            <div class="space-y-1 text-xs text-ink-muted">
                <p>{t("drift.lmStudio.description")}</p>
                <code class="block rounded-md border border-edge bg-overlay/50 px-2 py-1.5 text-[0.68rem] text-ink-faint">
                    http://127.0.0.1:1234
                </code>
            </div>
            <Show
                when={connected()}
                fallback={
                    <div class="rounded-md border border-warn/30 bg-warn/10 px-2.5 py-2 text-xs text-warn">
                        {t("drift.lmStudio.unavailable")}
                    </div>
                }
            >
                <div class="flex items-center justify-between text-xs">
                    <span class="text-ink-muted">{t("drift.lmStudio.discovered", { count: models().length })}</span>
                    <span class="rounded-full border border-ok/25 bg-ok/10 px-2 py-0.5 text-ok">
                        {t("drift.lmStudio.ready", { count: ready().length })}
                    </span>
                </div>
                <div class="max-h-52 space-y-1 overflow-y-auto">
                    <For each={models()}>
                        {(model) => {
                            const usable = () => lmStudioModelReady(model);
                            const lowContext = () =>
                                model.capabilities.toolcall && model.limit.context < lmStudioMinimumContext;

                            return (
                                <div class="flex items-center gap-2 rounded-md border border-edge bg-overlay/35 px-2.5 py-2">
                                    <span class="min-w-0 flex-1">
                                        <span class="block truncate text-xs text-ink">{model.name}</span>
                                        <span class="block truncate text-[0.65rem] text-ink-faint">{model.id}</span>
                                    </span>
                                    <span class="shrink-0 text-[0.65rem] tabular-nums text-ink-faint">
                                        {formatModelContext(model.limit.context)}
                                    </span>
                                    <span
                                        class="shrink-0 rounded-full border px-1.5 py-0.5 text-[0.62rem]"
                                        classList={{
                                            "border-ok/25 bg-ok/10 text-ok": usable(),
                                            "border-warn/25 bg-warn/10 text-warn": lowContext(),
                                            "border-edge text-ink-faint": !usable() && !lowContext(),
                                        }}
                                    >
                                        {t(lmStudioStatusLabel(usable(), lowContext()))}
                                    </span>
                                </div>
                            );
                        }}
                    </For>
                </div>
                <Show when={ready().length === 0}>
                    <div class="rounded-md border border-warn/30 bg-warn/10 px-2.5 py-2 text-xs text-warn">
                        {t("drift.lmStudio.noReady")}
                    </div>
                </Show>
            </Show>
            <div class="flex flex-wrap gap-2">
                <button
                    class="h-9 rounded-md bg-accent px-3.5 text-xs font-medium text-accent-ink transition-colors hover:brightness-105 disabled:opacity-40"
                    disabled={pending() !== null}
                    onClick={() => void refresh()}
                >
                    {pending() === "refresh" ? t("common.loading") : t("drift.lmStudio.refresh")}
                </button>
                <input
                    type="password"
                    class="h-9 min-w-44 flex-1 rounded-md border border-edge bg-overlay/50 px-2.5 text-sm outline-none transition-colors focus:border-edge-strong"
                    placeholder={t("drift.lmStudio.apiToken")}
                    value={key()}
                    onInput={(event) => setKey(event.currentTarget.value)}
                    onKeyDown={(event) => event.key === "Enter" && void saveToken()}
                />
                <button
                    class="h-9 rounded-md border border-edge px-3 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40"
                    disabled={pending() !== null || !key().trim()}
                    onClick={() => void saveToken()}
                >
                    {pending() === "token" ? t("common.loading") : t("common.save")}
                </button>
            </div>
        </div>
    );
}

function lmStudioStatusLabel(usable: boolean, lowContext: boolean) {
    if (usable) return "drift.lmStudio.modelReady";
    if (lowContext) return "drift.lmStudio.contextTooSmall";

    return "drift.lmStudio.notLoaded";
}
