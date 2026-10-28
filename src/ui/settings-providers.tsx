import { createEffect, createMemo, createSignal, For, Match, Show, Switch } from "solid-js";
import { hasProviderOptions, ProviderOptionsPanel } from "./settings-provider-options";
import { ProviderConnect, ProviderTab } from "./settings-provider-connect";
import { LmStudioConnect } from "./settings-lm-studio";
import { OllamaConnect } from "./settings-ollama";
import { ProviderIcon } from "./provider-icon";
import { useEngine } from "../engine";
import { Chevron } from "./controls";
import { t } from "../state/i18n";

import type { ProviderFamily, ProviderInfo } from "../engine/store";
import type { ProviderAuthMethod } from "./settings";
import type { JSX } from "solid-js";

export type ProviderNotice = { tone: "success" | "warning" | "error"; text: string };

/** One row in the list: a provider, or a maker's regions and plans together. */
type ProviderEntry = { id: string; name: string; routes: ProviderInfo[] };

/** Shown first until anything is connected: subscription sign-ins, then a free local runner. */
const suggestedIds = ["anthropic", "openai", "google", "ollama"];

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

    const entries = createMemo(() => {
        const byFamily = new Map<string, ProviderEntry>();
        for (const provider of engine.state.providers) {
            const id = provider.family?.id ?? provider.id;
            const entry = byFamily.get(id) ?? { id, name: provider.family?.name ?? provider.name, routes: [] };
            entry.routes.push(provider);
            byFamily.set(id, entry);
        }

        return [...byFamily.values()].map((entry) => ({ ...entry, routes: entry.routes.sort(byRoute) }));
    });

    const groups = createMemo(() => {
        const value = query().toLowerCase();
        const matching = entries()
            .filter((entry) =>
                [entry, ...entry.routes].some(
                    (item) => item.name.toLowerCase().includes(value) || item.id.toLowerCase().includes(value),
                ),
            )
            .sort((left, right) => left.name.localeCompare(right.name));
        const connected = (entry: ProviderEntry) =>
            entry.routes.some((route) => engine.state.connected.includes(route.id));

        const rest = matching.filter((entry) => !connected(entry));
        const first = engine.state.connected.length === 0 && !value;
        const suggested = first ? suggestedIds.flatMap((id) => rest.filter((entry) => entry.id === id)) : [];

        return {
            connected: matching.filter(connected),
            suggested,
            rest: rest.filter((entry) => !suggested.includes(entry)),
        };
    });

    const panel = (provider: ProviderInfo, tabs?: JSX.Element) => {
        const connected = () => engine.state.connected.includes(provider.id);

        return (
            <>
                <Switch
                    fallback={
                        <ProviderConnect
                            providerId={provider.id}
                            providerName={provider.name}
                            connected={connected()}
                            methods={
                                methods()[provider.id] ?? [{ type: "api", label: t("provider.connect.method.apiKey") }]
                            }
                            tabs={tabs}
                            onNotice={setNotice}
                        />
                    }
                >
                    <Match when={provider.id === "lmstudio"}>
                        <LmStudioConnect providerName={provider.name} onNotice={setNotice} />
                    </Match>
                    <Match when={provider.id === "ollama"}>
                        <OllamaConnect onNotice={setNotice} />
                    </Match>
                </Switch>
                <Show when={connected() && hasProviderOptions(provider.id)}>
                    <ProviderOptionsPanel providerId={provider.id} />
                </Show>
            </>
        );
    };

    const row = (entry: ProviderEntry) => {
        const connected = () => entry.routes.some((route) => engine.state.connected.includes(route.id));
        const open = () => expanded() === entry.id;

        return (
            <div
                class="overflow-hidden rounded-xl border transition-colors"
                classList={{ "border-edge bg-raised/25": open(), "border-transparent": !open() }}
            >
                <button
                    type="button"
                    class="group flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left transition-colors hover:bg-raised/60"
                    aria-expanded={open()}
                    onClick={() => setExpanded(open() ? null : entry.id)}
                >
                    <span class="flex size-8 shrink-0 items-center justify-center rounded-lg border border-edge bg-surface text-ink-muted shadow-sm shadow-black/10">
                        <ProviderIcon id={entry.id} class="size-4.5" />
                    </span>
                    <span class="min-w-0 flex-1 truncate text-sm font-medium text-ink">{entry.name}</span>
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
                    <Show when={entry.routes.length > 1} fallback={panel(entry.routes[0]!)}>
                        <ProviderRoutes routes={entry.routes} panel={panel} />
                    </Show>
                </Show>
            </div>
        );
    };

    return (
        <div class="space-y-1">
            <Show when={engine.state.connected.length === 0}>
                <p class="mb-3 px-1 text-sm text-ink-muted">{t("drift.settings.providers.intro")}</p>
            </Show>
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
                <div class="px-3 pt-1 pb-1 text-[length:calc(0.68rem*var(--settings-scale,1))] tracking-wider text-ink-faint uppercase">
                    {t("settings.providers.section.connected")}
                </div>
                <For each={groups().connected}>{row}</For>
            </Show>
            <Show when={groups().suggested.length > 0}>
                <div class="px-3 pt-1 pb-1 text-[length:calc(0.68rem*var(--settings-scale,1))] tracking-wider text-ink-faint uppercase">
                    {t("drift.settings.providers.suggested")}
                </div>
                <For each={groups().suggested}>{row}</For>
            </Show>
            <Show when={groups().rest.length > 0}>
                <div class="px-3 pt-3 pb-1 text-[length:calc(0.68rem*var(--settings-scale,1))] tracking-wider text-ink-faint uppercase">
                    {t(
                        groups().suggested.length
                            ? "drift.settings.providers.more"
                            : "drift.settings.providers.notConnected",
                    )}
                </div>
                <For each={groups().rest}>{row}</For>
            </Show>
            <Show when={groups().connected.length === 0 && groups().rest.length === 0}>
                <div class="px-3 py-4 text-sm text-ink-faint">{t("dialog.provider.empty")}</div>
            </Show>
        </div>
    );
}

/** A maker's routes under its one row, a tab each; the first connected one is shown first. */
function ProviderRoutes(props: {
    routes: ProviderInfo[];
    panel: (provider: ProviderInfo, tabs: JSX.Element) => JSX.Element;
}) {
    const engine = useEngine();
    const connected = (route: ProviderInfo) => engine.state.connected.includes(route.id);
    const [chosen, setChosen] = createSignal<string>();
    const route = () =>
        props.routes.find((item) => item.id === chosen()) ?? props.routes.find(connected) ?? props.routes[0]!;

    return (
        <Show when={route()} keyed>
            {(current) =>
                props.panel(
                    current,
                    <For each={props.routes}>
                        {(item) => (
                            <ProviderTab
                                active={item.id === current.id}
                                title={item.name}
                                onClick={() => setChosen(item.id)}
                            >
                                {routeLabel(item.family)}
                                <Show when={connected(item)}>
                                    <span class="size-1.5 rounded-full bg-ok" />
                                </Show>
                            </ProviderTab>
                        )}
                    </For>,
                )
            }
        </Show>
    );
}

function routeLabel(family: ProviderFamily | null | undefined) {
    if (family?.china) return t(family.plan ? "drift.providers.route.planChina" : "drift.providers.route.apiChina");

    return t(family?.plan ? "drift.providers.route.plan" : "drift.providers.route.api");
}

/** Pay-as-you-go before the plan, the global endpoint before China's. */
function byRoute(left: ProviderInfo, right: ProviderInfo) {
    const rank = (family: ProviderFamily | null | undefined) => (family?.china ? 2 : 0) + (family?.plan ? 1 : 0);

    return rank(left.family) - rank(right.family);
}
