import { SettingsGroup, SettingsRow, pickerWidth } from "./settings-controls";
import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { BrowserWorkspaceGroup } from "./settings-browser-workspace";
import { engineLabel } from "./browser-input";
import { useEngine } from "../engine";
import { Toggle } from "./controls";
import { t } from "../state/i18n";
import { Picker } from "./picker";

import type { BrowserKind, BrowserSettings, EngineStatus } from "../engine/native/client";

const downloadPollMs = 1000;
const button =
    "rounded-md border border-edge px-2.5 py-1 text-xs text-ink-muted transition-colors hover:bg-raised hover:text-ink disabled:opacity-40";

const kinds: BrowserKind[] = ["chrome", "edge", "firefox", "camoufox", "ie"];

/** Settings > Browser: the `browser` tool, the engines it can use, and the current workspace's setup. */
export function BrowserSection() {
    return (
        <div class="space-y-5">
            <BrowserGroup />
            <EnginesGroup />
            <BrowserWorkspaceGroup />
        </div>
    );
}

function errorText(cause: unknown) {
    return cause instanceof Error ? cause.message : String(cause);
}

/** The `browser` tool and how its browsers start; the engine owns the values. */
function BrowserGroup() {
    const engine = useEngine();
    // Null until the engine answers, so no control shows a guess.
    const [settings, setSettings] = createSignal<BrowserSettings | null>(null);
    const [error, setError] = createSignal("");

    onMount(
        () =>
            void engine.actions
                .engineSettings()
                .then((loaded) => setSettings(loaded.browser ?? null))
                .catch(() => undefined),
    );

    /** Shows the change at once and takes it back if the engine refuses it. */
    async function change(patch: Partial<BrowserSettings>) {
        const previous = settings();
        if (!previous) return;

        const next = { ...previous, ...patch };
        setSettings(next);
        setError("");
        await engine.actions.setBrowserSettings(next).catch((cause) => {
            setSettings(previous);
            setError(errorText(cause));
        });
    }

    const picker = (props: {
        label: string;
        items: { id: string; label: string }[];
        selected: string;
        onPick: (id: string) => void;
    }) => (
        <Picker
            label={props.label}
            items={props.items}
            selected={props.selected}
            floating
            bordered
            chevronAtEnd
            placement="below"
            width={pickerWidth}
            onPick={props.onPick}
        />
    );

    return (
        <SettingsGroup title={t("drift.settings.browser.tool")}>
            <Show when={settings()}>
                {(current) => (
                    <>
                        <SettingsRow
                            title={t("drift.settings.browser.enabled.title")}
                            description={t("drift.settings.browser.enabled.description")}
                            onClick={() => void change({ enabled: !current().enabled })}
                        >
                            <Toggle
                                label={t("drift.settings.browser.enabled.title")}
                                checked={current().enabled}
                                onChange={() => void change({ enabled: !current().enabled })}
                            />
                        </SettingsRow>
                        <SettingsRow
                            title={t("drift.settings.browser.engine.title")}
                            description={t("drift.settings.browser.engine.description")}
                            disabled={!current().enabled}
                        >
                            {picker({
                                label: t("drift.settings.browser.engine.title"),
                                items: kinds.map((kind) => ({ id: kind, label: engineLabel(kind) })),
                                selected: current().engine,
                                onPick: (id) => void change({ engine: id as BrowserKind }),
                            })}
                        </SettingsRow>
                        <SettingsRow
                            title={t("drift.settings.browser.storage.title")}
                            description={t("drift.settings.browser.storage.description")}
                            disabled={!current().enabled}
                        >
                            {picker({
                                label: t("drift.settings.browser.storage.title"),
                                items: [
                                    { id: "isolated", label: t("drift.settings.browser.storage.isolated") },
                                    { id: "shared", label: t("drift.settings.browser.storage.shared") },
                                ],
                                selected: current().storage,
                                onPick: (id) => void change({ storage: id as BrowserSettings["storage"] }),
                            })}
                        </SettingsRow>
                        <SettingsRow
                            title={t("drift.settings.browser.window.title")}
                            description={t("drift.settings.browser.window.description")}
                            disabled={!current().enabled}
                        >
                            {picker({
                                label: t("drift.settings.browser.window.title"),
                                items: [
                                    { id: "hidden", label: t("drift.settings.browser.window.hidden") },
                                    { id: "offscreen", label: t("drift.settings.browser.window.offscreen") },
                                ],
                                selected: current().window,
                                onPick: (id) => void change({ window: id as BrowserSettings["window"] }),
                            })}
                        </SettingsRow>
                    </>
                )}
            </Show>
            <Show when={error()}>
                <div class="text-xs text-danger">{error()}</div>
            </Show>
        </SettingsGroup>
    );
}

/** Each engine: whether it can run here, where its program is, and fetching the ones Drift can. */
function EnginesGroup() {
    const engine = useEngine();
    const [engines, setEngines] = createSignal<EngineStatus[]>([]);
    const [error, setError] = createSignal("");

    const load = () =>
        engine.actions
            .browserEngines()
            .then(setEngines)
            .catch((cause) => setError(errorText(cause)));
    onMount(() => void load());

    // A download reports its progress only when asked, so the list is read again while one runs.
    const timer = setInterval(() => {
        if (engines().some((status) => status.download && !status.download.error)) void load();
    }, downloadPollMs);
    onCleanup(() => clearInterval(timer));

    async function download(kind: BrowserKind) {
        setError("");
        await engine.actions
            .downloadBrowserEngine(kind)
            .then(load)
            .catch((cause) => setError(errorText(cause)));
    }

    async function setPath(kind: BrowserKind, path: string | null) {
        setError("");
        await engine.actions
            .setBrowserEnginePath(kind, path)
            .then(setEngines)
            .catch((cause) => setError(errorText(cause)));
    }

    return (
        <SettingsGroup title={t("drift.settings.browser.engines")}>
            <For each={engines()}>
                {(status) => (
                    <EngineRow status={status} onDownload={() => void download(status.engine)} onPath={setPath} />
                )}
            </For>
            <Show when={error()}>
                <div class="py-2 text-xs text-danger">{error()}</div>
            </Show>
        </SettingsGroup>
    );
}

function engineDescription(status: EngineStatus) {
    const download = status.download;
    if (download?.error) return t("drift.settings.browser.downloadFailed", { error: download.error });
    if (download) {
        const percent = download.total ? Math.floor((download.done / download.total) * 100) : 0;
        return t("drift.settings.browser.downloading", { percent });
    }
    if (status.available) return status.path ?? t("drift.settings.browser.ready");

    return status.missing ?? t("drift.settings.browser.unavailable");
}

function EngineRow(props: {
    status: EngineStatus;
    onDownload: () => void;
    onPath: (kind: BrowserKind, path: string | null) => Promise<void>;
}) {
    const [editing, setEditing] = createSignal(false);
    const [draft, setDraft] = createSignal("");
    const busy = () => !!props.status.download && !props.status.download.error;
    const pathLabel = () =>
        props.status.engine === "ie" ? t("drift.settings.browser.driverPath") : t("drift.settings.browser.path");

    async function save(path: string | null) {
        await props.onPath(props.status.engine, path);
        setEditing(false);
    }

    return (
        <>
            <SettingsRow title={engineLabel(props.status.engine)} description={engineDescription(props.status)}>
                <div class="flex items-center gap-2">
                    <span
                        class="text-xs"
                        classList={{ "text-ok": props.status.available, "text-ink-faint": !props.status.available }}
                    >
                        {props.status.available
                            ? t("drift.settings.browser.ready")
                            : t("drift.settings.browser.missing")}
                    </span>
                    <Show when={props.status.downloadable && !props.status.available}>
                        <button class={button} disabled={busy()} onClick={() => props.onDownload()}>
                            {t("drift.settings.browser.download")}
                        </button>
                    </Show>
                    <button
                        class={button}
                        onClick={() => {
                            setDraft(props.status.custom ? (props.status.path ?? "") : "");
                            setEditing(!editing());
                        }}
                    >
                        {pathLabel()}
                    </button>
                </div>
            </SettingsRow>
            <Show when={editing()}>
                <div class="flex items-center gap-2 border-b border-edge/70 px-1 py-2.5">
                    <input
                        class="min-w-0 flex-1 rounded-md border border-edge bg-surface px-2.5 py-1.5 font-mono text-xs text-ink outline-none focus:border-edge-strong"
                        aria-label={pathLabel()}
                        placeholder={t("drift.settings.browser.pathPlaceholder")}
                        value={draft()}
                        onInput={(event) => setDraft(event.currentTarget.value)}
                    />
                    <button class={button} onClick={() => void save(draft().trim() || null)}>
                        {t("common.save")}
                    </button>
                    <Show when={props.status.custom}>
                        <button class={button} onClick={() => void save(null)}>
                            {t("drift.settings.browser.findItself")}
                        </button>
                    </Show>
                </div>
            </Show>
        </>
    );
}
