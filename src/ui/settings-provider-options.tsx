import { createSignal, onMount, Show } from "solid-js";
import { SettingsRow } from "./settings-controls";
import { useEngine } from "../engine";
import { Toggle } from "./controls";
import { t } from "../state/i18n";

import type { components } from "../engine/native/types";

type ProviderOptions = components["schemas"]["ProviderOptions"];

/** Mirrors the engine's `HOUR_CACHE_PROVIDERS`: the routes that cache Claude for an hour when asked. */
const hourCacheProviders = ["anthropic", "amazon-bedrock", "google-vertex-anthropic", "google-vertex", "openrouter"];

/** Whether a provider has options of its own to show. */
export function hasProviderOptions(providerId: string) {
    return hourCacheProviders.includes(providerId);
}

/** Settings > Providers > a provider: the options the engine keeps for it. */
export function ProviderOptionsPanel(props: { providerId: string }) {
    const engine = useEngine();
    // Null until the engine answers, so a toggle never shows a guess.
    const [options, setOptions] = createSignal<ProviderOptions | null>(null);
    const [error, setError] = createSignal("");

    onMount(
        () =>
            void engine.actions
                .engineSettings()
                .then((settings) => setOptions(settings.providerOptions?.[props.providerId] ?? { hourCache: false }))
                .catch(() => undefined),
    );

    /** Shows the change at once and takes it back if the engine refuses it. */
    async function change(patch: Partial<ProviderOptions>) {
        const before = options();
        if (!before) return;
        const next = { ...before, ...patch };
        setOptions(next);
        setError("");
        try {
            const saved = await engine.actions.putEngineSettings({ providerOptions: { [props.providerId]: next } });
            setOptions(saved.providerOptions?.[props.providerId] ?? next);
        } catch (cause) {
            setOptions(before);
            setError(cause instanceof Error ? cause.message : String(cause));
        }
    }

    const hourDescription = () => {
        const description = t("drift.providers.options.hourCache.description");
        if (props.providerId !== "anthropic") return description;

        return `${description} ${t("drift.providers.options.hourCache.subscription")}`;
    };

    return (
        <Show when={options()}>
            {(current) => (
                <div class="border-t border-edge/70 px-3 pt-1 pb-2">
                    <div class="pt-2 text-[length:calc(0.68rem*var(--settings-scale,1))] tracking-wider text-ink-faint uppercase">
                        {t("drift.providers.options.title")}
                    </div>
                    <Show when={hourCacheProviders.includes(props.providerId)}>
                        <SettingsRow
                            title={t("drift.providers.options.hourCache.title")}
                            description={hourDescription()}
                            onClick={() => void change({ hourCache: !current().hourCache })}
                        >
                            <Toggle
                                label={t("drift.providers.options.hourCache.title")}
                                checked={current().hourCache}
                                onChange={() => void change({ hourCache: !current().hourCache })}
                            />
                        </SettingsRow>
                    </Show>
                    <Show when={error()}>
                        <div class="text-xs text-danger">{error()}</div>
                    </Show>
                </div>
            )}
        </Show>
    );
}
