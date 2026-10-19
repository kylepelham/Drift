import { SettingsGroup, SettingsRow } from "./settings-controls";
import { createSignal, onMount, Show } from "solid-js";
import { useEngine } from "../engine";
import { t } from "../state/i18n";
import { Picker } from "./picker";

import type { components } from "../engine/native/types";

type ImageGenerationState = components["schemas"]["ImageGenerationState"];
type ImageGeneration = components["schemas"]["ImageGeneration"];

/** The sizes offered as the default; the agent can still ask for any ratio or pixel size itself. */
const sizes = ["auto", "1:1", "3:2", "2:3", "16:9", "9:16"];

/** What the provider row says: what the switch does, or that the chosen provider still needs an API key. */
export function imageProviderNote(state: ImageGenerationState) {
    const chosen = state.providers.find((provider) => provider.id === state.settings.provider);
    if (chosen && !chosen.hasKey) return t("drift.settings.imageGeneration.noKey", { provider: chosen.name });

    return t("drift.settings.imageGeneration.provider.description");
}

/** Settings > Execution > Image generation: off at first, else the provider, its model and the default size. */
export function ImageGenerationGroup() {
    const engine = useEngine();
    const [state, setState] = createSignal<ImageGenerationState | null>(null);
    const [error, setError] = createSignal("");

    async function run(action: Promise<ImageGenerationState>) {
        setError("");
        try {
            setState(await action);
        } catch (cause) {
            setError(cause instanceof Error ? cause.message : String(cause));
        }
    }

    onMount(() => void run(engine.actions.imageGeneration()));

    const save = (patch: Partial<ImageGeneration>) =>
        void run(engine.actions.putImageGeneration({ ...state()!.settings, ...patch }));
    const chosen = () => state()?.providers.find((provider) => provider.id === state()?.settings.provider);
    const picker = { floating: true, bordered: true, chevronAtEnd: true, placement: "below" as const };

    return (
        <Show when={state()}>
            {(current) => (
                <SettingsGroup title={t("drift.settings.imageGeneration")}>
                    <SettingsRow
                        title={t("drift.settings.imageGeneration.provider.title")}
                        description={imageProviderNote(current())}
                    >
                        <Picker
                            {...picker}
                            label={t("drift.settings.imageGeneration.provider.title")}
                            items={[
                                { id: "off", label: t("drift.settings.imageGeneration.off") },
                                ...current().providers.map((provider) => ({
                                    id: provider.id,
                                    label: provider.name,
                                    hint: provider.hasKey ? undefined : t("drift.settings.imageGeneration.needsKey"),
                                })),
                            ]}
                            selected={current().settings.provider}
                            width="9rem"
                            onPick={(provider) => save({ provider, model: "" })}
                        />
                    </SettingsRow>
                    <Show when={chosen()}>
                        {(provider) => (
                            <>
                                <SettingsRow
                                    title={t("drift.settings.imageGeneration.model.title")}
                                    description={t("drift.settings.imageGeneration.model.description")}
                                >
                                    <Picker
                                        {...picker}
                                        label={t("drift.settings.imageGeneration.model.title")}
                                        items={provider().models.map((model) => ({ id: model, label: model }))}
                                        selected={current().settings.model || provider().models[0]}
                                        width="14rem"
                                        onPick={(model) => save({ model })}
                                    />
                                </SettingsRow>
                                <SettingsRow
                                    title={t("drift.settings.imageGeneration.size.title")}
                                    description={t("drift.settings.imageGeneration.size.description")}
                                >
                                    <Picker
                                        {...picker}
                                        label={t("drift.settings.imageGeneration.size.title")}
                                        items={sizes.map((size) => ({
                                            id: size,
                                            label:
                                                size === "auto" ? t("drift.settings.imageGeneration.size.auto") : size,
                                        }))}
                                        selected={current().settings.size}
                                        width="7rem"
                                        onPick={(size) => save({ size })}
                                    />
                                </SettingsRow>
                            </>
                        )}
                    </Show>
                    <Show when={error()}>
                        <div class="text-xs text-danger">{error()}</div>
                    </Show>
                </SettingsGroup>
            )}
        </Show>
    );
}
