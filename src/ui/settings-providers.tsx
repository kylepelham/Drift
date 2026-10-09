import { createEffect, createMemo, createSignal, For, Show } from "solid-js";
import { ProviderConnect } from "./settings-provider-connect";
import { LmStudioConnect } from "./settings-lm-studio";
import { ProviderIcon } from "./provider-icon";
import { useEngine } from "../engine";
import { Chevron } from "./controls";
import { t } from "../state/i18n";

import type { ProviderAuthMethod } from "./settings";

export type ProviderNotice = { tone: "success" | "warning" | "error"; text: string };

export function ProvidersSection() {
    const engine = useEngine();
    const [methods, setMethods] = createSignal<Record<string, ProviderAuthMethod[]>>({});
    const [expanded, setExpanded] = createSignal<string | null>(null);
    const [query, setQuery] = createSignal("");
    const [notice, setNotice] = createSignal<ProviderNotice | null>(null);

    createEffect(() => {
        // Health can report a version before a workspace starts the event pump.
        if (engine.state.connection !== "online" && !engine.state.version) return;

        void engine.actions
            .providerAuthMethods()
            .then((map) => setMethods({ ...map }))
            .catch(() => {});
    });

    const groups = createMemo(() => {
        const value = query().toLowerCase();
        const matching = engine.state.providers
            .filter(
                (provider) => provider.name.toLowerCase().includes(value) || provider.id.toLowerCase().includes(value),
            )
            .sort((left, right) => left.name.localeCompare(right.name));

        return {
            connected: matching.filter((provider) => engine.state.connected.includes(provider.id)),
            rest: matching.filter((provider) => !engine.state.connected.includes(provider.id)),
        };
    });

    const row = (provider: (typeof engine.state.providers)[number]) => {
        const connected = () => engine.state.connected.includes(provider.id);
        const open = () => expanded() === provider.id;

        return (
            <div
                class="overflow-hidden rounded-xl border transition-colors"
                classList={{ "border-edge bg-raised/25": open(), "border-transparent": !open() }}
            >
                <button
                    type="button"
                    class="group flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left transition-colors hover:bg-raised/60"
                    aria-expanded={open()}
                    onClick={() => setExpanded(open() ? null : provider.id)}
                >
                    <span class="flex size-8 shrink-0 items-center justify-center rounded-lg border border-edge bg-surface text-ink-muted shadow-sm shadow-black/10">
                        <ProviderIcon id={provider.id} class="size-4.5" />
                    </span>
                    <span class="min-w-0 flex-1 truncate text-sm font-medium text-ink">{provider.name}</span>
                    {/* Group headings already identify connection status, so the row needs only a dot. */}
                    <Show when={connected()}>
                        <span
                            role="img"
                            aria-label={t("mcp.status.connected")}
                            title={t("mcp.status.connected")}
                            class="size-1.5 shrink-0 rounded-full bg-ok"
                        />
                    </Show>
                    <span class="text-ink-faint transition-colors group-hover:text-ink-muted">
                        <Chevron open={open()} />
                    </span>
                </button>
                <Show when={open()}>
                    {provider.id === "lmstudio" ? (
                        <LmStudioConnect providerName={provider.name} onNotice={setNotice} />
                    ) : (
                        <ProviderConnect
                            providerId={provider.id}
                            providerName={provider.name}
                            connected={connected()}
                            methods={
                                methods()[provider.id] ?? [{ type: "api", label: t("provider.connect.method.apiKey") }]
                            }
                            onNotice={setNotice}
                        />
                    )}
                </Show>
            </div>
        );
    };

    return (
        <div class="space-y-1">
            <input
                class="mb-2 h-9 w-full rounded-md border border-edge bg-raised/45 px-2.5 text-sm text-ink outline-none transition-colors placeholder:text-ink-faint focus:border-accent"
                placeholder={t("dialog.provider.search.placeholder")}
                value={query()}
                onInput={(event) => setQuery(event.currentTarget.value)}
            />
            <Show when={notice()}>
                {(item) => (
                    <div
                        class="mb-2 rounded-md border px-3 py-2 text-xs"
                        classList={{
                            "border-ok/35 bg-ok/10 text-ok": item().tone === "success",
                            "border-warn/35 bg-warn/10 text-warn": item().tone === "warning",
                            "border-danger/35 bg-danger/10 text-danger": item().tone === "error",
                        }}
                    >
                        {item().text}
                    </div>
                )}
            </Show>
            <Show when={groups().connected.length > 0}>
                <div class="px-3 pt-1 pb-1 text-[0.68rem] tracking-wider text-ink-faint uppercase">
                    {t("settings.providers.section.connected")}
                </div>
                <For each={groups().connected}>{row}</For>
            </Show>
            <Show when={groups().rest.length > 0}>
                <div class="px-3 pt-3 pb-1 text-[0.68rem] tracking-wider text-ink-faint uppercase">
                    {t("drift.settings.providers.notConnected")}
                </div>
                <For each={groups().rest}>{row}</For>
            </Show>
            <Show when={groups().connected.length === 0 && groups().rest.length === 0}>
                <div class="px-3 py-4 text-sm text-ink-faint">{t("dialog.provider.empty")}</div>
            </Show>
        </div>
    );
}
