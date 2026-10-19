import { SettingsGroup, SettingsRow, pickerWidth } from "./settings-controls";
import { createSignal, onMount, Show } from "solid-js";
import { useEngine } from "../engine";
import { t } from "../state/i18n";
import { Picker } from "./picker";

import type { components } from "../engine/native/types";

type ImageGenerationState = components["schemas"]["ImageGenerationState"];
type ImageGeneration = components["schemas"]["ImageGeneration"];
type ImageProvider = components["schemas"]["ImageProvider"];

/** The sizes offered as the default; the agent can still ask for any ratio or pixel size itself. */
const sizes = ["auto", "1:1", "3:2", "2:3", "16:9", "9:16"];
const input =
    "h-8 rounded-md border border-edge bg-raised/45 px-2.5 text-xs text-ink outline-none placeholder:text-ink-faint focus:border-accent";
const button = "h-8 rounded-md border border-edge px-3 text-xs text-ink-muted hover:text-ink disabled:opacity-40";
const picker = { floating: true, bordered: true, chevronAtEnd: true, placement: "below" as const, width: pickerWidth };

/** What the provider row says: what the switch does, or what the chosen provider still needs. */
export function imageProviderNote(state: ImageGenerationState) {
    const chosen = state.providers.find((provider) => provider.id === state.settings.provider);
    if (chosen && !chosen.ready) return t("drift.settings.imageGeneration.noKey", { provider: chosen.name });
    if (chosen?.hasPlan && !chosen.hasKey) return t("drift.settings.imageGeneration.planOnly");

    return t("drift.settings.imageGeneration.provider.description");
}

/** What an empty typed model means for the provider. */
function typedModelNote(provider: ImageProvider) {
    if (provider.id === "comfyui") return t("drift.settings.imageGeneration.model.firstCheckpoint");
    if (provider.id === "sdwebui") return t("drift.settings.imageGeneration.model.loaded");
    if (provider.models[0])
        return t("drift.settings.imageGeneration.model.typedDefault", { model: provider.models[0] });

    return t("drift.settings.imageGeneration.model.required");
}

/** Settings > Execution > Image generation: off at first, else the provider, who pays, its model and the default size. */
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
    const saveKey = (provider: string, key: string) => run(engine.actions.setImageKey(provider, key));
    const chosen = () => state()?.providers.find((provider) => provider.id === state()?.settings.provider);

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
                                    hint: provider.ready ? undefined : t("drift.settings.imageGeneration.needsKey"),
                                })),
                            ]}
                            selected={current().settings.provider}
                            onPick={(provider) => save({ provider, model: "" })}
                        />
                    </SettingsRow>
                    <Show when={chosen()}>
                        {(provider) => (
                            <>
                                <Show when={provider().hasPlan && provider().hasKey}>
                                    <PaymentRow
                                        first={current().settings.openaiFirst}
                                        onPick={(openaiFirst) => save({ openaiFirst })}
                                    />
                                </Show>
                                <Show when={provider().server !== undefined && provider().server !== null}>
                                    <ServerRow
                                        provider={provider()}
                                        onSave={(address) =>
                                            save({
                                                servers: { ...current().settings.servers, [provider().id]: address },
                                            })
                                        }
                                    />
                                </Show>
                                <Show when={provider().access !== "server" || provider().id === "compatible"}>
                                    <KeyRow provider={provider()} onSave={(key) => saveKey(provider().id, key)} />
                                </Show>
                                <ModelRow
                                    provider={provider()}
                                    model={current().settings.model}
                                    onSave={(model) => save({ model })}
                                />
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

/** OpenAI with both a ChatGPT sign-in and an API key: which pays first; the other takes over when it is refused or spent. */
function PaymentRow(props: { first: ImageGeneration["openaiFirst"]; onPick: (first: "plan" | "apiKey") => void }) {
    return (
        <SettingsRow
            title={t("drift.settings.imageGeneration.payFirst.title")}
            description={t("drift.settings.imageGeneration.payFirst.description")}
        >
            <Picker
                {...picker}
                label={t("drift.settings.imageGeneration.payFirst.title")}
                items={[
                    { id: "plan", label: t("drift.settings.imageGeneration.payFirst.plan") },
                    { id: "apiKey", label: t("drift.settings.imageGeneration.payFirst.apiKey") },
                ]}
                selected={props.first}
                onPick={(first) => props.onPick(first === "apiKey" ? "apiKey" : "plan")}
            />
        </SettingsRow>
    );
}

function keyNote(provider: ImageProvider) {
    if (provider.fromEnvironment) return t("drift.settings.imageGeneration.key.environment");
    if (provider.access === "catalog") return t("drift.settings.imageGeneration.key.catalogDescription");
    if (provider.access === "server") return t("drift.settings.imageGeneration.key.serverDescription");

    return t("drift.settings.imageGeneration.key.description");
}

/** A key saved for images: entered once, then shown only by its last characters. */
function KeyRow(props: { provider: ImageProvider; onSave: (key: string) => Promise<void> }) {
    const [draft, setDraft] = createSignal("");
    const save = async () => {
        if (!draft().trim()) return;
        await props.onSave(draft());
        setDraft("");
    };

    return (
        <SettingsRow title={t("drift.settings.imageGeneration.key.title")} description={keyNote(props.provider)}>
            <Show
                when={props.provider.key}
                fallback={
                    <div class="flex items-center gap-2">
                        <input
                            type="password"
                            aria-label={t("drift.settings.imageGeneration.key.placeholder")}
                            placeholder={t("drift.settings.imageGeneration.key.placeholder")}
                            class={`w-40 ${input}`}
                            value={draft()}
                            onInput={(event) => setDraft(event.currentTarget.value)}
                            onKeyDown={(event) => event.key === "Enter" && void save()}
                        />
                        <button class={button} disabled={!draft().trim()} onClick={() => void save()}>
                            {t("drift.settings.imageGeneration.key.save")}
                        </button>
                    </div>
                }
            >
                {(key) => (
                    <div class="flex items-center gap-2">
                        <span class="rounded-md bg-raised px-2 py-1 font-mono text-xs text-ink-muted">{key()}</span>
                        <button class={button} onClick={() => void props.onSave("")}>
                            {t("drift.settings.imageGeneration.key.remove")}
                        </button>
                    </div>
                )}
            </Show>
        </SettingsRow>
    );
}

/** A local or self-hosted server's address, saved when the field is left. */
function ServerRow(props: { provider: ImageProvider; onSave: (address: string) => void }) {
    return (
        <SettingsRow
            title={t("drift.settings.imageGeneration.server.title")}
            description={t("drift.settings.imageGeneration.server.description", { provider: props.provider.name })}
        >
            <input
                type="text"
                aria-label={t("drift.settings.imageGeneration.server.title")}
                class={`w-56 font-mono ${input}`}
                value={props.provider.server ?? ""}
                onChange={(event) => props.onSave(event.currentTarget.value.trim())}
            />
        </SettingsRow>
    );
}

/** A picked model from the provider's list, or a typed endpoint or checkpoint, saved when the field is left. */
function ModelRow(props: { provider: ImageProvider; model: string; onSave: (model: string) => void }) {
    return (
        <SettingsRow
            title={t("drift.settings.imageGeneration.model.title")}
            description={
                props.provider.typed
                    ? typedModelNote(props.provider)
                    : t("drift.settings.imageGeneration.model.description")
            }
        >
            <Show
                when={props.provider.typed}
                fallback={
                    <Picker
                        {...picker}
                        label={t("drift.settings.imageGeneration.model.title")}
                        items={props.provider.models.map((model) => ({ id: model, label: model }))}
                        selected={props.model || props.provider.models[0]}
                        onPick={(model) => props.onSave(model)}
                    />
                }
            >
                <input
                    type="text"
                    aria-label={t("drift.settings.imageGeneration.model.title")}
                    placeholder={props.provider.models[0] ?? ""}
                    class={`w-56 font-mono ${input}`}
                    value={props.model}
                    onChange={(event) => props.onSave(event.currentTarget.value.trim())}
                />
            </Show>
        </SettingsRow>
    );
}
