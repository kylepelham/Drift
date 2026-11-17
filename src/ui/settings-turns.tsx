import { SettingsGroup, SettingsRow } from "./settings-controls";
import { createSignal, onMount, type Setter } from "solid-js";
import { useEngine } from "../engine";
import { Toggle } from "./controls";
import { t } from "../state/i18n";

/** How turns go on: started again after Drift restarts, and a cyber refusal retried once. The engine keeps both. */
export function TurnsGroup() {
    const engine = useEngine();
    // Null until the engine answers, so a toggle never shows a guess.
    const [resume, setResume] = createSignal<boolean | null>(null);
    const [retry, setRetry] = createSignal<boolean | null>(null);

    onMount(
        () =>
            void engine.actions
                .engineSettings()
                .then((settings) => {
                    setResume(settings.resumeTurns ?? true);
                    setRetry(settings.retryRefusals ?? false);
                })
                .catch(() => undefined),
    );

    async function toggle(current: boolean | null, set: Setter<boolean | null>, key: "resumeTurns" | "retryRefusals") {
        if (current === null) return;
        set(!current);
        await engine.actions.putEngineSettings({ [key]: !current }).catch(() => set(current));
    }

    return (
        <SettingsGroup title={t("drift.settings.execution.turns")}>
            <TurnToggle
                title={t("drift.settings.resumeTurns.title")}
                description={t("drift.settings.resumeTurns.description")}
                value={resume()}
                onToggle={() => void toggle(resume(), setResume, "resumeTurns")}
            />
            <TurnToggle
                title={t("drift.settings.retryRefusals.title")}
                description={t("drift.settings.retryRefusals.description")}
                value={retry()}
                onToggle={() => void toggle(retry(), setRetry, "retryRefusals")}
            />
        </SettingsGroup>
    );
}

function TurnToggle(props: { title: string; description: string; value: boolean | null; onToggle: () => void }) {
    return (
        <SettingsRow
            title={props.title}
            description={props.description}
            disabled={props.value === null}
            onClick={() => props.onToggle()}
        >
            <Toggle
                label={props.title}
                checked={!!props.value}
                disabled={props.value === null}
                onChange={() => props.onToggle()}
            />
        </SettingsRow>
    );
}
