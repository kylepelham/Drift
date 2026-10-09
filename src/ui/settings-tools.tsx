import { createSignal, onCleanup, onMount, Show } from "solid-js";
import { SettingsGroup, SettingsRow } from "./settings-controls";
import { useEngine } from "../engine";
import { t } from "../state/i18n";
import { Picker } from "./picker";
import {
    setShellTimeoutMs,
    listenShellTimeoutError,
    shellTimeoutMaxMs,
    shellTimeoutMinMs,
    shellTimeoutMs,
    shellTimeoutPresets,
} from "../state/prefs";

export function ToolExecutionSection() {
    const isPreset = (value: number | null) =>
        value === null || (shellTimeoutPresets as readonly number[]).includes(value);
    const [customOpen, setCustomOpen] = createSignal(!isPreset(shellTimeoutMs()));
    const [customMinutes, setCustomMinutes] = createSignal(
        String(isPreset(shellTimeoutMs()) ? 10 : shellTimeoutMs()! / 60_000),
    );
    const [error, setError] = createSignal("");

    onMount(() => {
        const stop = listenShellTimeoutError(setError);
        onCleanup(stop);
    });

    const customValue = () => Number(customMinutes()) * 60_000;
    const customValid = () =>
        Number.isInteger(Number(customMinutes())) &&
        customValue() >= shellTimeoutMinMs &&
        customValue() <= shellTimeoutMaxMs;
    const selected = () => (customOpen() || !isPreset(shellTimeoutMs()) ? "custom" : String(shellTimeoutMs()));

    async function applyTimeout(value: number | null) {
        setError("");
        await setShellTimeoutMs(value).catch((cause) =>
            setError(cause instanceof Error ? cause.message : String(cause)),
        );
    }

    return (
        <div class="space-y-5">
            <SettingsGroup title={t("drift.settings.execution.shell")}>
                <SettingsRow
                    title={t("drift.settings.shellTimeout.title")}
                    description={t("drift.settings.shellTimeout.description")}
                >
                    <Picker
                        label={t("drift.settings.shellTimeout.title")}
                        items={[
                            { id: "null", label: t("drift.settings.shellTimeout.noTimeout") },
                            ...shellTimeoutPresets.map((value) => ({
                                id: String(value),
                                label: t(`drift.settings.shellTimeout.preset${value / 60_000}`),
                            })),
                            { id: "custom", label: t("drift.settings.shellTimeout.custom") },
                        ]}
                        selected={selected()}
                        floating
                        bordered
                        chevronAtEnd
                        placement="below"
                        width="12rem"
                        onPick={(id) => {
                            if (id === "custom") return setCustomOpen(true);

                            setCustomOpen(false);
                            void applyTimeout(id === "null" ? null : Number(id));
                        }}
                    />
                </SettingsRow>
                <Show when={customOpen()}>
                    <SettingsRow
                        title={t("drift.settings.shellTimeout.customMinutes")}
                        description={
                            customValid()
                                ? t("drift.settings.shellTimeout.customDescription")
                                : t("drift.settings.shellTimeout.invalid")
                        }
                    >
                        <div class="flex items-center gap-2">
                            <input
                                type="number"
                                min={shellTimeoutMinMs / 60_000}
                                max={shellTimeoutMaxMs / 60_000}
                                step="1"
                                aria-label={t("drift.settings.shellTimeout.customMinutes")}
                                aria-invalid={!customValid()}
                                class="w-24 rounded-md border border-edge bg-surface px-2.5 py-1.5 text-right font-mono text-xs text-ink outline-none focus:border-edge-strong"
                                value={customMinutes()}
                                onInput={(event) => {
                                    setCustomMinutes(event.currentTarget.value);
                                }}
                            />
                            <button
                                class="rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-white transition-opacity hover:opacity-90 disabled:opacity-40"
                                disabled={!customValid()}
                                onClick={() => void applyTimeout(customValue())}
                            >
                                {t("common.save")}
                            </button>
                        </div>
                    </SettingsRow>
                </Show>
            </SettingsGroup>
            <Show when={error()}>
                <div class="text-xs text-danger">{error()}</div>
            </Show>
            <BackgroundLimitGroup />
        </div>
    );
}

const backgroundLimits = Array.from({ length: 16 }, (_, index) => index + 1);

/** How many background subagents the engine runs at once; the engine owns the value. */
function BackgroundLimitGroup() {
    const engine = useEngine();
    const [limit, setLimit] = createSignal<number | null>(null);
    const [error, setError] = createSignal("");

    // Null until the engine answers, so the picker never shows a guess.
    onMount(
        () =>
            void engine.actions
                .engineSettings()
                .then((settings) => setLimit(settings.backgroundTaskLimit ?? null))
                .catch(() => undefined),
    );

    async function pick(next: number) {
        const previous = limit();
        setLimit(next);
        setError("");

        try {
            const settings = await engine.actions.setBackgroundTaskLimit(next);
            setLimit(settings.backgroundTaskLimit ?? next);
        } catch (cause) {
            setLimit(previous);
            setError(cause instanceof Error ? cause.message : String(cause));
        }
    }

    return (
        <SettingsGroup title={t("drift.settings.execution.subagents")}>
            <SettingsRow
                title={t("drift.settings.backgroundLimit.title")}
                description={t("drift.settings.backgroundLimit.description")}
            >
                <Show when={limit()}>
                    {(current) => (
                        <Picker
                            label={t("drift.settings.backgroundLimit.title")}
                            items={backgroundLimits.map((value) => ({ id: String(value), label: String(value) }))}
                            selected={String(current())}
                            floating
                            bordered
                            chevronAtEnd
                            placement="below"
                            width="6rem"
                            onPick={(id) => void pick(Number(id))}
                        />
                    )}
                </Show>
            </SettingsRow>
            <Show when={error()}>
                <div class="text-xs text-danger">{error()}</div>
            </Show>
        </SettingsGroup>
    );
}
