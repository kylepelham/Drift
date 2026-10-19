import { SettingsGroup, SettingsRow } from "./settings-controls";
import { createSignal, onMount } from "solid-js";
import { useEngine } from "../engine";
import { Toggle } from "./controls";
import { t } from "../state/i18n";

/** Whether a turn Drift stopped in the middle of starts again when Drift does; the engine keeps it, on at first. */
export function TurnsGroup() {
    const engine = useEngine();
    // Null until the engine answers, so the toggle never shows a guess.
    const [resume, setResume] = createSignal<boolean | null>(null);

    onMount(
        () =>
            void engine.actions
                .engineSettings()
                .then((settings) => setResume(settings.resumeTurns ?? true))
                .catch(() => undefined),
    );

    async function toggle() {
        const next = !resume();
        setResume(next);
        await engine.actions.putEngineSettings({ resumeTurns: next }).catch(() => setResume(!next));
    }

    return (
        <SettingsGroup title={t("drift.settings.execution.turns")}>
            <SettingsRow
                title={t("drift.settings.resumeTurns.title")}
                description={t("drift.settings.resumeTurns.description")}
                disabled={resume() === null}
                onClick={() => resume() !== null && void toggle()}
            >
                <Toggle
                    label={t("drift.settings.resumeTurns.title")}
                    checked={!!resume()}
                    disabled={resume() === null}
                    onChange={() => void toggle()}
                />
            </SettingsRow>
        </SettingsGroup>
    );
}
