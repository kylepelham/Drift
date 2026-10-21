import { SettingsGroup, SettingsRow } from "./settings-controls";
import { createSignal, onMount, Show } from "solid-js";
import { useEngine } from "../engine";
import { Toggle } from "./controls";
import { t } from "../state/i18n";

/** Settings > Execution: whether large servers' tools load on request with `tool_search`, kept by the engine. */
export function ToolSearchGroup() {
    const engine = useEngine();
    // Null until the engine answers, so the toggle never shows a guess.
    const [on, setOn] = createSignal<boolean | null>(null);
    const [error, setError] = createSignal("");

    onMount(
        () =>
            void engine.actions
                .engineSettings()
                .then((settings) => setOn(settings.toolSearch ?? true))
                .catch(() => undefined),
    );

    async function toggle() {
        const before = on();
        if (before === null) return;
        setOn(!before);
        setError("");
        try {
            const saved = await engine.actions.putEngineSettings({ toolSearch: !before });
            setOn(saved.toolSearch ?? !before);
        } catch (cause) {
            setOn(before);
            setError(cause instanceof Error ? cause.message : String(cause));
        }
    }

    return (
        <SettingsGroup title={t("drift.settings.toolSearch.group")}>
            <SettingsRow
                title={t("drift.settings.toolSearch.title")}
                description={t("drift.settings.toolSearch.description")}
                disabled={on() === null}
                onClick={() => void toggle()}
            >
                <Toggle
                    label={t("drift.settings.toolSearch.title")}
                    checked={on() ?? false}
                    disabled={on() === null}
                    onChange={() => void toggle()}
                />
            </SettingsRow>
            <Show when={error()}>
                <div class="text-xs text-danger">{error()}</div>
            </Show>
        </SettingsGroup>
    );
}
