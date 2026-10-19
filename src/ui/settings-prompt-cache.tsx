import { SettingsGroup, SettingsRow } from "./settings-controls";
import { createSignal, onMount, Show } from "solid-js";
import { useEngine } from "../engine";
import { Toggle } from "./controls";
import { t } from "../state/i18n";

import type { components } from "../engine/native/types";

type PromptCache = components["schemas"]["PromptCache"];

const input =
    "rounded-md border border-edge bg-surface px-2.5 py-1.5 text-xs text-ink outline-none focus:border-edge-strong";

/** Settings > Execution > Prompt cache: warming and Anthropic's hour-long cache, kept by the engine, all off at first. */
export function PromptCacheGroup() {
    const engine = useEngine();
    // Null until the engine answers, so a toggle never shows a guess.
    const [cache, setCache] = createSignal<PromptCache | null>(null);
    const [error, setError] = createSignal("");

    onMount(
        () =>
            void engine.actions
                .engineSettings()
                .then((settings) => setCache(settings.promptCache ?? null))
                .catch(() => undefined),
    );

    /** Shows the change at once and takes it back if the engine refuses it. */
    async function change(patch: Partial<PromptCache>) {
        const before = cache();
        if (!before) return;
        const next = { ...before, ...patch };
        setCache(next);
        setError("");
        try {
            const saved = await engine.actions.putEngineSettings({ promptCache: next });
            setCache(saved.promptCache ?? next);
        } catch (cause) {
            setCache(before);
            setError(cause instanceof Error ? cause.message : String(cause));
        }
    }

    const minutes = (key: "intervalMinutes" | "windowMinutes", label: string) => (
        <input
            type="number"
            min="1"
            step="1"
            aria-label={label}
            class={`w-20 text-right font-mono ${input}`}
            value={cache()?.[key] ?? ""}
            onChange={(event) => void change({ [key]: event.currentTarget.valueAsNumber })}
        />
    );

    return (
        <Show when={cache()}>
            {(current) => (
                <SettingsGroup title={t("drift.settings.promptCache")}>
                    <SettingsRow
                        title={t("drift.settings.promptCache.warming.title")}
                        description={t("drift.settings.promptCache.warming.description")}
                        onClick={() => void change({ warming: !current().warming })}
                    >
                        <Toggle
                            label={t("drift.settings.promptCache.warming.title")}
                            checked={current().warming}
                            onChange={() => void change({ warming: !current().warming })}
                        />
                    </SettingsRow>
                    <Show when={current().warming}>
                        <SettingsRow
                            title={t("drift.settings.promptCache.interval.title")}
                            description={t("drift.settings.promptCache.interval.description")}
                        >
                            {minutes("intervalMinutes", t("drift.settings.promptCache.interval.title"))}
                        </SettingsRow>
                        <SettingsRow
                            title={t("drift.settings.promptCache.window.title")}
                            description={t("drift.settings.promptCache.window.description")}
                        >
                            {minutes("windowMinutes", t("drift.settings.promptCache.window.title"))}
                        </SettingsRow>
                        <SettingsRow
                            title={t("drift.settings.promptCache.prompt.title")}
                            description={t("drift.settings.promptCache.prompt.description")}
                        >
                            <input
                                type="text"
                                aria-label={t("drift.settings.promptCache.prompt.title")}
                                placeholder={t("drift.settings.promptCache.prompt.placeholder")}
                                class={`w-64 ${input}`}
                                value={current().prompt}
                                onChange={(event) => void change({ prompt: event.currentTarget.value })}
                            />
                        </SettingsRow>
                    </Show>
                    <SettingsRow
                        title={t("drift.settings.promptCache.hour.title")}
                        description={t("drift.settings.promptCache.hour.description")}
                        onClick={() => void change({ hourCache: !current().hourCache })}
                    >
                        <Toggle
                            label={t("drift.settings.promptCache.hour.title")}
                            checked={current().hourCache}
                            onChange={() => void change({ hourCache: !current().hourCache })}
                        />
                    </SettingsRow>
                    <Show when={error()}>
                        <div class="text-xs text-danger">{error()}</div>
                    </Show>
                </SettingsGroup>
            )}
        </Show>
    );
}
