import { SettingsGroup, SettingsRow } from "./settings-controls";
import { createSignal, Match, Show, Switch } from "solid-js";
import { systemVoices } from "../voice/read-aloud";
import { Toggle } from "./controls";
import { t } from "../state/i18n";
import { Picker } from "./picker";
import {
    openaiVoices,
    readAloudAuto,
    readAloudEnabled,
    setReadAloudAuto,
    setReadAloudEnabled,
    setSpeechProvider,
    setSpeechVoice,
    speechProvider,
    speechProviders,
    speechVoices,
    type SpeechProvider,
} from "../state/voice";

import type { components } from "../engine/native/types";

export type VoiceProvider = components["schemas"]["VoiceProvider"];
export type VoiceKeys = {
    providers: VoiceProvider[];
    save: (provider: string, key: string) => Promise<void>;
    remove: (provider: string) => Promise<void>;
};

const input =
    "h-8 rounded-md border border-edge bg-raised/45 px-2.5 text-xs text-ink outline-none placeholder:text-ink-faint focus:border-accent";
const button = "h-8 rounded-md border border-edge px-3 text-xs text-ink-muted hover:text-ink disabled:opacity-40";
const picker = { floating: true, bordered: true, chevronAtEnd: true, placement: "below", width: "14rem" } as const;

/** A cloud provider's key: entered once, then shown only by its last characters. */
export function VoiceKeyRow(props: { provider: VoiceProvider; keys: VoiceKeys }) {
    const [draft, setDraft] = createSignal("");
    const save = async () => {
        if (!draft().trim()) return;
        await props.keys.save(props.provider.id, draft());
        setDraft("");
    };

    return (
        <SettingsRow
            title={t("drift.voice.key.title", { provider: props.provider.name })}
            description={props.provider.key ? t("drift.voice.key.description") : t("drift.voice.key.none")}
        >
            <Show
                when={props.provider.key}
                fallback={
                    <div class="flex items-center gap-2">
                        <input
                            type="password"
                            aria-label={t("drift.voice.key.placeholder")}
                            placeholder={t("drift.voice.key.placeholder")}
                            class={`w-40 ${input}`}
                            value={draft()}
                            onInput={(event) => setDraft(event.currentTarget.value)}
                            onKeyDown={(event) => event.key === "Enter" && void save()}
                        />
                        <button class={button} disabled={!draft().trim()} onClick={() => void save()}>
                            {t("drift.voice.key.save")}
                        </button>
                    </div>
                }
            >
                {(key) => (
                    <div class="flex items-center gap-2">
                        <span class="rounded-md bg-raised px-2 py-1 font-mono text-xs text-ink-muted">{key()}</span>
                        <button class={button} onClick={() => void props.keys.remove(props.provider.id)}>
                            {t("drift.voice.key.remove")}
                        </button>
                    </div>
                )}
            </Show>
        </SettingsRow>
    );
}

function VoiceRow() {
    return (
        <SettingsRow title={t("drift.voice.read.voice.title")} description={t("drift.voice.read.voice.description")}>
            <Switch>
                <Match when={speechProvider() === "local"}>
                    <Picker
                        label={t("drift.voice.read.voice.title")}
                        items={[
                            { id: "", label: t("drift.voice.read.voice.system") },
                            ...systemVoices().map((voice) => ({
                                id: voice.voiceURI,
                                label: voice.name,
                                detail: voice.lang,
                            })),
                        ]}
                        selected={speechVoices().local ?? ""}
                        {...picker}
                        onPick={(value) => setSpeechVoice("local", value)}
                    />
                </Match>
                <Match when={speechProvider() === "openai"}>
                    <Picker
                        label={t("drift.voice.read.voice.title")}
                        items={openaiVoices.map((id) => ({ id, label: id[0]!.toUpperCase() + id.slice(1) }))}
                        selected={speechVoices().openai ?? openaiVoices[0]}
                        {...picker}
                        onPick={(value) => setSpeechVoice("openai", value)}
                    />
                </Match>
                <Match when={speechProvider() === "elevenlabs"}>
                    <input
                        aria-label={t("drift.voice.read.voice.title")}
                        placeholder={t("drift.voice.read.voice.idPlaceholder")}
                        class={`w-full sm:w-56 ${input}`}
                        value={speechVoices().elevenlabs ?? ""}
                        onChange={(event) => setSpeechVoice("elevenlabs", event.currentTarget.value)}
                    />
                </Match>
            </Switch>
        </SettingsRow>
    );
}

/** Settings > Voice > Read aloud: the play button, who speaks, the voice, and the auto-read switch. */
export function ReadAloudGroup(props: { keys: VoiceKeys }) {
    const cloud = (id: SpeechProvider) =>
        props.keys.providers.find((provider) => provider.id === id && provider.speech);
    const chosen = () => (speechProvider() === "local" ? undefined : cloud(speechProvider()));

    return (
        <>
            <SettingsGroup title={t("drift.voice.read")}>
                <SettingsRow
                    title={t("drift.voice.read.enabled.title")}
                    description={t("drift.voice.read.enabled.description")}
                    onClick={() => setReadAloudEnabled(!readAloudEnabled())}
                >
                    <Toggle
                        label={t("drift.voice.read.enabled.title")}
                        checked={readAloudEnabled()}
                        onChange={() => setReadAloudEnabled(!readAloudEnabled())}
                    />
                </SettingsRow>
                <Show when={readAloudEnabled()}>
                    <SettingsRow
                        title={t("drift.voice.read.provider.title")}
                        description={t("drift.voice.read.provider.description")}
                    >
                        <Picker
                            label={t("drift.voice.read.provider.title")}
                            items={speechProviders
                                .filter((id) => id === "local" || cloud(id))
                                .map((id) => ({
                                    id,
                                    label: id === "local" ? t("drift.voice.read.provider.local") : cloud(id)!.name,
                                }))}
                            selected={speechProvider()}
                            {...picker}
                            onPick={(value) => setSpeechProvider(value as SpeechProvider)}
                        />
                    </SettingsRow>
                    <Show when={chosen()}>{(provider) => <VoiceKeyRow provider={provider()} keys={props.keys} />}</Show>
                    <VoiceRow />
                    <SettingsRow
                        title={t("drift.voice.read.auto.title")}
                        description={t("drift.voice.read.auto.description")}
                        onClick={() => setReadAloudAuto(!readAloudAuto())}
                    >
                        <Toggle
                            label={t("drift.voice.read.auto.title")}
                            checked={readAloudAuto()}
                            onChange={() => setReadAloudAuto(!readAloudAuto())}
                        />
                    </SettingsRow>
                </Show>
            </SettingsGroup>
            <Show when={readAloudEnabled()}>
                <div class="px-1 text-[length:calc(0.72rem*var(--settings-scale,1))] leading-relaxed text-ink-faint">
                    {chosen()
                        ? t("drift.voice.read.privacy.cloud", { provider: chosen()!.name })
                        : t("drift.voice.read.privacy.local")}
                </div>
            </Show>
        </>
    );
}
