import { SettingsGroup, SettingsRow, pickerWidth } from "./settings-controls";
import { createSignal, onMount, Show } from "solid-js";
import { useEngine } from "../engine";
import { Toggle } from "./controls";
import { t } from "../state/i18n";
import { Picker } from "./picker";

import type { BrowserSettings } from "../engine/native/client";

/** Settings > Browser: the `browser` tool and how its browsers start. */
export function BrowserSection() {
    return (
        <div class="space-y-5">
            <BrowserGroup />
        </div>
    );
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
            setError(cause instanceof Error ? cause.message : String(cause));
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
        <SettingsGroup title={t("drift.settings.browser.title")}>
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
                                items: [
                                    { id: "chrome", label: "Chrome" },
                                    { id: "edge", label: "Edge" },
                                ],
                                selected: current().engine,
                                onPick: (id) => void change({ engine: id as BrowserSettings["engine"] }),
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
