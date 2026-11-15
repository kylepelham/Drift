import { SettingsGroup, SettingsRow, pickerWidth } from "./settings-controls";
import { createSignal, For, onMount, Show } from "solid-js";
import { IconArrowUp, IconTrash } from "./icons";
import { useEngine } from "../engine";
import { Chevron } from "./controls";
import { t } from "../state/i18n";
import { Picker } from "./picker";

import type { components } from "../engine/native/types";

type WebSearchState = components["schemas"]["WebSearchState"];
type SearchProvider = components["schemas"]["SearchProvider"];
type KeyCheck = components["schemas"]["KeyCheck"];

const input =
    "rounded-md border border-edge bg-surface px-2.5 py-1.5 text-xs text-ink outline-none focus:border-edge-strong";
const button = "rounded-md border border-edge px-2.5 py-1 text-xs text-ink-muted hover:text-ink disabled:opacity-40";
const iconButton =
    "flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink";

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
    const [checks, setChecks] = createSignal<KeyCheck[] | null>(null);
    const [checking, setChecking] = createSignal(false);

    async function run(action: Promise<WebSearchState>) {
        setError("");
        try {
            setState(await action);
        } catch (cause) {
            setError(cause instanceof Error ? cause.message : String(cause));
        }
    }

    onMount(() => void run(engine.actions.webSearch()));

    async function check() {
        setChecking(true);
        setError("");
        try {
            setChecks(await engine.actions.checkWebSearchKeys());
        } catch (cause) {
            setError(cause instanceof Error ? cause.message : String(cause));
        } finally {
            setChecking(false);
        }
    }

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
                            width={pickerWidth}
                            onPick={choose}
                        />
                    </SettingsRow>
                    <Show when={current().settings.provider !== "off"}>
                        <SettingsRow
                            title={t("drift.settings.webSearch.credits.title")}
                            description={t("drift.settings.webSearch.credits.description")}
                        >
                            <button class={button} disabled={checking()} onClick={() => void check()}>
                                {t(
                                    checking()
                                        ? "drift.settings.webSearch.credits.checking"
                                        : "drift.settings.webSearch.credits.check",
                                )}
                            </button>
                        </SettingsRow>
                        <For each={orderedProviders(current())}>
                            {(provider, index) => (
                                <ProviderRow
                                    provider={provider}
                                    checks={(checks() ?? []).filter((check) => check.provider === provider.id)}
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

/** A provider's line under its name: where its keys come from, or how many it has. */
function providerNote(provider: SearchProvider) {
    if (!provider.builtIn) return t("drift.settings.webSearch.plugin");
    if (provider.fromEnvironment) return t("drift.settings.webSearch.environment");
    if (provider.keys.length === 0) return t("drift.settings.webSearch.noKey");

    const count = provider.keys.length;
    return t(count === 1 ? "drift.settings.webSearch.keys.one" : "drift.settings.webSearch.keys.other", { count });
}

/** A provider's line after a check: what its keys have left together, then any that failed. */
export function creditSummary(provider: SearchProvider, checks: KeyCheck[]) {
    const counted = checks.filter((check) => check.state === "ok" && check.balance);
    const left = counted.reduce((sum, check) => sum + check.balance!.left, 0);
    const failing = (state: KeyCheck["state"]) => checks.filter((check) => check.state === state).length;
    const parts = [
        counted.length > 0 ? t("drift.settings.webSearch.credits.left", { left: left.toLocaleString() }) : "",
        counted.length === 0 && failing("ok") > 0
            ? t("drift.settings.webSearch.credits.works", { provider: provider.name })
            : "",
        failing("refused") > 0 ? t("drift.settings.webSearch.credits.refused", { count: failing("refused") }) : "",
        failing("rateLimited") > 0 ? t("drift.settings.webSearch.credits.busy", { count: failing("rateLimited") }) : "",
        failing("failed") > 0 ? t("drift.settings.webSearch.credits.failed", { count: failing("failed") }) : "",
    ];

    return parts.filter(Boolean).join(" \u00b7 ");
}

/** One key's check as a short line: its plan, what it has left and when that renews, or why the check failed. */
export function keyResult(check: KeyCheck) {
    const balance = check.balance;
    if (check.state === "ok" && balance) {
        const left = balance.total
            ? t("drift.settings.webSearch.credits.ofTotal", {
                  left: balance.left.toLocaleString(),
                  total: balance.total.toLocaleString(),
              })
            : t("drift.settings.webSearch.credits.left", { left: balance.left.toLocaleString() });
        const renews = balance.renews
            ? t("drift.settings.webSearch.credits.renews", { date: new Date(balance.renews).toLocaleDateString() })
            : "";
        return [balance.plan ?? "", left, renews].filter(Boolean).join(" \u00b7 ");
    }
    if (check.state === "ok") return t("drift.settings.webSearch.credits.keyWorks");
    const reason = {
        refused: "drift.settings.webSearch.credits.keyRefused",
        rateLimited: "drift.settings.webSearch.credits.keyBusy",
        failed: "drift.settings.webSearch.credits.keyFailed",
    } as const;
    return [t(reason[check.state]), check.error ?? ""].filter(Boolean).join(": ");
}

/** The colour a key's check is shown in: a problem stands out, a spent key warns. */
function resultTone(check: KeyCheck) {
    if (check.state === "refused" || check.state === "failed") return "text-danger";
    if (check.state === "rateLimited" || check.balance?.left === 0) return "text-warn";
    return "text-ink-faint";
}

/** A provider as one row, its keys listed in a panel it opens. */
function ProviderRow(props: {
    provider: SearchProvider;
    checks: KeyCheck[];
    canMoveUp: boolean;
    onMoveUp: () => void;
    onAdd: (key: string) => Promise<void>;
    onRemove: (index: number) => Promise<void>;
}) {
    const [open, setOpen] = createSignal(false);
    const [draft, setDraft] = createSignal("");
    const add = async () => {
        if (!draft().trim()) return;
        await props.onAdd(draft());
        setDraft("");
    };

    return (
        <div class="border-b border-edge/70 last:border-b-0 [&>.settings-row]:border-b-0">
            <SettingsRow
                title={props.provider.name}
                description={
                    props.checks.length > 0 ? creditSummary(props.provider, props.checks) : providerNote(props.provider)
                }
                onClick={props.provider.builtIn ? () => setOpen(!open()) : undefined}
            >
                <div class="flex items-center gap-1">
                    <Show when={props.canMoveUp}>
                        <button
                            class={iconButton}
                            aria-label={t("drift.settings.webSearch.moveUp")}
                            title={t("drift.settings.webSearch.moveUp")}
                            onClick={(event) => {
                                event.stopPropagation();
                                props.onMoveUp();
                            }}
                        >
                            <IconArrowUp class="size-3.5" />
                        </button>
                    </Show>
                    <Show when={props.provider.builtIn}>
                        <button
                            class={iconButton}
                            aria-expanded={open()}
                            aria-label={t("drift.settings.webSearch.keys.manage", { provider: props.provider.name })}
                            onClick={(event) => {
                                event.stopPropagation();
                                setOpen(!open());
                            }}
                        >
                            <Chevron open={open()} />
                        </button>
                    </Show>
                </div>
            </SettingsRow>
            <Show when={open()}>
                <div class="mx-1 mb-3 overflow-hidden rounded-lg border border-edge bg-surface/55">
                    <For each={props.provider.keys}>
                        {(key, index) => (
                            <div class="group flex items-center gap-3 border-b border-edge/60 px-3 py-1.5">
                                <span class="min-w-0 flex-1 font-mono text-xs text-ink-muted">
                                    {"\u2022".repeat(4)} {key.replace(/^\.+/, "")}
                                </span>
                                <Show when={props.checks.find((check) => check.index === index())}>
                                    {(check) => (
                                        <span class={`min-w-0 truncate text-xs ${resultTone(check())}`}>
                                            {keyResult(check())}
                                        </span>
                                    )}
                                </Show>
                                <button
                                    class={`${iconButton} opacity-0 group-hover:opacity-100 focus-visible:opacity-100`}
                                    aria-label={t("drift.settings.webSearch.removeKey", { key })}
                                    title={t("drift.settings.webSearch.removeKey", { key })}
                                    onClick={() => void props.onRemove(index())}
                                >
                                    <IconTrash class="size-3.5" />
                                </button>
                            </div>
                        )}
                    </For>
                    <Show when={props.checks.find((check) => check.index == null)}>
                        {(check) => (
                            <div class="flex items-center gap-3 border-b border-edge/60 px-3 py-1.5">
                                <span class="min-w-0 flex-1 text-xs text-ink-muted">
                                    {t("drift.settings.webSearch.credits.environment")}
                                </span>
                                <span class={`min-w-0 truncate text-xs ${resultTone(check())}`}>
                                    {keyResult(check())}
                                </span>
                            </div>
                        )}
                    </Show>
                    <div class="flex items-center gap-2 p-2">
                        <input
                            type="password"
                            aria-label={t("drift.settings.webSearch.keyPlaceholder")}
                            placeholder={t("drift.settings.webSearch.keyPlaceholder")}
                            class={`min-w-0 flex-1 ${input}`}
                            value={draft()}
                            onInput={(event) => setDraft(event.currentTarget.value)}
                            onKeyDown={(event) => event.key === "Enter" && void add()}
                        />
                        <button class={button} disabled={!draft().trim()} onClick={() => void add()}>
                            {t("drift.settings.webSearch.addKey")}
                        </button>
                    </div>
                </div>
            </Show>
        </div>
    );
}
