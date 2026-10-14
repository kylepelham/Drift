import { createSignal, onMount, Show } from "solid-js";
import { SettingsRow } from "./settings-controls";
import { useEngine } from "../engine";
import { Toggle } from "./controls";
import { t } from "../state/i18n";

/** Whether agents may run commands in the background, and whether a command at its limit moves there. */
export function createShellSettings() {
    const engine = useEngine();
    // Null until the engine answers, so a toggle never shows a guess.
    const [processes, setProcesses] = createSignal<boolean | null>(null);
    const [handoff, setHandoff] = createSignal<boolean | null>(null);
    const [error, setError] = createSignal("");

    onMount(
        () =>
            void engine.actions
                .engineSettings()
                .then((settings) => {
                    setProcesses(settings.backgroundProcesses ?? true);
                    setHandoff(settings.shellHandoff ?? true);
                })
                .catch(() => undefined),
    );

    /** Shows the change at once and takes it back if the engine refuses it. */
    async function change(shown: (on: boolean) => void, next: boolean, save: (on: boolean) => Promise<unknown>) {
        shown(next);
        setError("");
        await save(next).catch((cause) => {
            shown(!next);
            setError(cause instanceof Error ? cause.message : String(cause));
        });
    }

    return {
        processes,
        handoff,
        error,
        /** Hand-off works only while background processes are on, since the agent could not see the process otherwise. */
        handsOff: () => !!processes() && !!handoff(),
        toggleProcesses: () => void change(setProcesses, !processes(), engine.actions.setBackgroundProcesses),
        toggleHandoff: () => void change(setHandoff, !handoff(), engine.actions.setShellHandoff),
    };
}

export type ShellSettings = ReturnType<typeof createShellSettings>;

/** The two switches, as rows of the Shell group. */
export function BackgroundRows(props: { settings: ShellSettings }) {
    const settings = () => props.settings;

    return (
        <>
            <SettingsRow
                title={t("drift.settings.backgroundProcesses.title")}
                description={t("drift.settings.backgroundProcesses.description")}
                disabled={settings().processes() === null}
                onClick={settings().toggleProcesses}
            >
                <Toggle
                    label={t("drift.settings.backgroundProcesses.title")}
                    checked={!!settings().processes()}
                    disabled={settings().processes() === null}
                    onChange={settings().toggleProcesses}
                />
            </SettingsRow>
            <SettingsRow
                title={t("drift.settings.shellHandoff.title")}
                description={t("drift.settings.shellHandoff.description")}
                disabled={!settings().processes()}
                onClick={settings().toggleHandoff}
            >
                <Toggle
                    label={t("drift.settings.shellHandoff.title")}
                    checked={settings().handsOff()}
                    disabled={!settings().processes()}
                    onChange={settings().toggleHandoff}
                />
            </SettingsRow>
            <Show when={settings().error()}>
                <div class="text-xs text-danger">{settings().error()}</div>
            </Show>
        </>
    );
}
