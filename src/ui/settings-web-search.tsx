import { SettingsGroup, SettingsRow } from "./settings-controls";
import { createSignal, For, onMount, Show } from "solid-js";
import { useEngine } from "../engine";
import { t } from "../state/i18n";
import { Picker } from "./picker";

import type { components } from "../engine/native/types";

type WebSearchState = components["schemas"]["WebSearchState"];
type SearchProvider = components["schemas"]["SearchProvider"];

const input =
    "rounded-md border border-edge bg-surface px-2.5 py-1.5 text-xs text-ink outline-none focus:border-edge-strong";
const button = "rounded-md border border-edge px-2.5 py-1 text-xs text-ink-muted hover:text-ink disabled:opacity-40";

/** The providers in the order `auto` tries them: the saved order, then the rest as the engine lists them. */
export function orderedProviders(state: WebSearchState): SearchProvider[] {
    const rank = (id: string) => {
        const at = state.settings.order.indexOf(id);
        return at === -1 ? state.settings.order.length : at;
    };

    return state.providers
        .map((provider, index) => ({ provider, index }))
        .sort((a, b) => rank(a.provider.id) - rank(b.provider.id) || a.index - b.index)
        .map(({ provider }) => provider);
}

/** Settings > Execution > Web search: which provider answers `websearch`, and each built-in's keys. */
export function WebSearchGroup() {
    const engine = useEngine();
    const [state, setState] = createSignal<WebSearchState | null>(null);
    const [error, setError] = createSignal("");

    async function run(action: Promise<WebSearchState>) {
        setError("");
        try {
            setState(await action);
        } catch (cause) {
            setError(cause instanceof Error ? cause.message : String(cause));
        }
    }

    onMount(() => void run(engine.actions.webSearch()));

    const choose = (provider: string) => void run(engine.actions.putWebSearch({ ...state()!.settings, provider }));
    const moveUp = (id: string) => {
        const order = orderedProviders(state()!).map((provider) => provider.id);
        const at = order.indexOf(id);
        order.splice(at - 1, 0, ...order.splice(at, 1));
        void run(engine.actions.putWebSearch({ ...state()!.settings, order }));
    };

    return (
        <Show when={state()}>
            {(current) => (
                <SettingsGroup title={t("drift.settings.webSearch")}>
                    <SettingsRow
                        title={t("drift.settings.webSearch.provider.title")}
                        description={t("drift.settings.webSearch.provider.description")}
                    >
                        <Picker
                            label={t("drift.settings.webSearch.provider.title")}
                            items={[
                                { id: "auto", label: t("drift.settings.webSearch.auto") },
                                ...current().providers.map((provider) => ({ id: provider.id, label: provider.name })),
                                { id: "off", label: t("drift.settings.webSearch.off") },
                            ]}
                            selected={current().settings.provider}
                            floating
                            bordered
                            chevronAtEnd
                            placement="below"
                            width="9rem"
                            onPick={choose}
                        />
                    </SettingsRow>
                    <Show when={current().settings.provider !== "off"}>
                        <For each={orderedProviders(current())}>
                            {(provider, index) => (
                                <ProviderRow
                                    provider={provider}
                                    canMoveUp={current().settings.provider === "auto" && index() > 0}
                                    onMoveUp={() => moveUp(provider.id)}
                                    onAdd={(key) => run(engine.actions.addWebSearchKey(provider.id, key))}
                                    onRemove={(at) => run(engine.actions.removeWebSearchKey(provider.id, at))}
                                />
                            )}
                        </For>
                    </Show>
                    <Show when={error()}>
                        <div class="text-xs text-danger">{error()}</div>
                    </Show>
                </SettingsGroup>
            )}
        </Show>
    );
}

function providerNote(provider: SearchProvider) {
    if (!provider.builtIn) return t("drift.settings.webSearch.plugin");
    if (provider.fromEnvironment) return t("drift.settings.webSearch.environment");
    return provider.keys.length ? "" : t("drift.settings.webSearch.noKey");
}

function ProviderRow(props: {
    provider: SearchProvider;
    canMoveUp: boolean;
    onMoveUp: () => void;
    onAdd: (key: string) => Promise<void>;
    onRemove: (index: number) => Promise<void>;
}) {
    const [draft, setDraft] = createSignal("");
    const add = async () => {
        if (!draft().trim()) return;
        await props.onAdd(draft());
        setDraft("");
    };

    return (
        <div class="border-b border-edge/70 last:border-b-0 [&>.settings-row]:border-b-0">
            <SettingsRow title={props.provider.name} description={providerNote(props.provider)}>
                <div class="flex items-center justify-end gap-2">
                    <Show when={props.provider.builtIn}>
                        <input
                            type="password"
                            aria-label={t("drift.settings.webSearch.keyPlaceholder")}
                            placeholder={t("drift.settings.webSearch.keyPlaceholder")}
                            class={`w-40 ${input}`}
                            value={draft()}
                            onInput={(event) => setDraft(event.currentTarget.value)}
                            onKeyDown={(event) => event.key === "Enter" && void add()}
                        />
                        <button class={button} disabled={!draft().trim()} onClick={() => void add()}>
                            {t("drift.settings.webSearch.addKey")}
                        </button>
                    </Show>
                    <Show when={props.canMoveUp}>
                        <button
                            class={button}
                            aria-label={t("drift.settings.webSearch.moveUp")}
                            title={t("drift.settings.webSearch.moveUp")}
                            onClick={() => props.onMoveUp()}
                        >
                            ↑
                        </button>
                    </Show>
                </div>
            </SettingsRow>
            {/* Keys get their own line, which wraps, so however many there are the row keeps its title. */}
            <Show when={props.provider.keys.length > 0}>
                <div class="-mt-1 flex flex-wrap gap-1.5 px-1 pb-2.5">
                    <For each={props.provider.keys}>
                        {(key, index) => (
                            <span class="flex items-center gap-1 rounded-md bg-raised px-2 py-1 font-mono text-xs text-ink-muted">
                                {key}
                                <button
                                    class="text-ink-faint hover:text-ink"
                                    aria-label={t("drift.settings.webSearch.removeKey", { key })}
                                    onClick={() => void props.onRemove(index())}
                                >
                                    ×
                                </button>
                            </span>
                        )}
                    </For>
                </div>
            </Show>
        </div>
    );
}
