import { ReadAloudGroup, VoiceKeyRow, type VoiceKeys, type VoiceProvider } from "./settings-read-aloud";
import { audioInputDevices, selectedDeviceMissing, watchAudioInputDevices } from "../voice/devices";
import { setDictationEnabled, syncDictationConsent } from "../voice/dictation";
import { SettingsGroup, SettingsRow, pickerWidth } from "./settings-controls";
import { createSignal, onCleanup, onMount, Show } from "solid-js";
import { watchLocalVoices } from "../voice/read-aloud";
import { useEngine } from "../engine";
import { Toggle } from "./controls";
import { t } from "../state/i18n";
import { Picker } from "./picker";
import {
    dictationEnabled,
    dictationInputDeviceId,
    dictationKeyterms,
    dictationLanguage,
    dictationLanguages,
    dictationModel,
    dictationModels,
    dictationProvider,
    dictationProviders,
    formatKeyterms,
    parseKeyterms,
    setDictationInputDeviceId,
    setDictationKeyterms,
    setDictationLanguage,
    setDictationModel,
    setDictationProvider,
    type DictationLanguage,
    type DictationModel,
    type DictationProvider,
} from "../state/voice";
import {
    cancelVoiceModelDownload,
    downloadPercent,
    downloadVoiceModel,
    formatBytes,
    modelInfo,
    refreshVoiceModels,
    removeVoiceModel,
    voiceAccelerated,
    voiceModelBusy,
    voiceModelError,
    voiceProgress,
    voiceSupported,
    type VoiceModelInfo,
} from "../voice/models";

const languageLabels: Record<DictationLanguage, string> = {
    auto: "",
    en: "English",
    es: "Español",
    fr: "Français",
    de: "Deutsch",
    hi: "हिन्दी",
    it: "Italiano",
    ja: "日本語",
    nl: "Nederlands",
    pt: "Português",
    ru: "Русский",
};

const modelLabels: Record<DictationModel, string> = {
    "large-v3-turbo-q5_0": "drift.voice.model.best",
    "small-q5_1": "drift.voice.model.balanced",
    "base-q5_1": "drift.voice.model.fastest",
};

/** Sizes come from the shell, so the row stays neutral until it has answered. */
function storageDescription(model: VoiceModelInfo | undefined) {
    if (!model) return t("common.loading");
    if (model.installed) return t("drift.voice.model.storage.ready");
    return t("drift.voice.model.storage.missing", { size: formatBytes(model.bytes) });
}

/** The cloud voice providers and their keys, kept by the engine. */
function createVoiceKeys(): VoiceKeys & { error: () => string; load: () => void } {
    const engine = useEngine();
    const [providers, setProviders] = createSignal<VoiceProvider[]>([]);
    const [error, setError] = createSignal("");
    const run = async (action: Promise<{ providers: VoiceProvider[] }>) => {
        setError("");
        try {
            setProviders((await action).providers);
        } catch (cause) {
            setError(cause instanceof Error ? cause.message : String(cause));
        }
    };

    return {
        get providers() {
            return providers();
        },
        error,
        load: () => void run(engine.actions.voice()),
        save: (provider, key) => run(engine.actions.setVoiceKey(provider, key)),
        remove: (provider) => run(engine.actions.removeVoiceKey(provider)),
    };
}

export function VoiceSection() {
    const [keyterms, setKeyterms] = createSignal(formatKeyterms(dictationKeyterms()));
    const keys = createVoiceKeys();
    onMount(() => {
        void refreshVoiceModels();
        void syncDictationConsent().catch(() => undefined);
        keys.load();
        onCleanup(watchAudioInputDevices());
        onCleanup(watchLocalVoices());
    });

    const selected = () => modelInfo(dictationModel());
    const downloading = () => voiceModelBusy() === "downloading";
    const local = () => dictationProvider() === "local";
    const cloud = () =>
        keys.providers.find((provider) => provider.id === dictationProvider() && provider.transcription);
    const providerLabel = (id: DictationProvider) =>
        id === "local"
            ? t("drift.voice.provider.local")
            : (keys.providers.find((provider) => provider.id === id)?.name ?? id);

    function commitKeyterms(value: string) {
        const parsed = parseKeyterms(value);
        setDictationKeyterms(parsed);
        setKeyterms(formatKeyterms(parsed));
    }

    return (
        <div class="space-y-6">
            <SettingsGroup title={t("drift.voice.dictation")}>
                <SettingsRow
                    title={t("drift.voice.dictation.enabled.title")}
                    description={t("drift.voice.dictation.enabled.description")}
                    onClick={() => void setDictationEnabled(!dictationEnabled())}
                >
                    <Toggle
                        label={t("drift.voice.dictation.enabled.title")}
                        checked={dictationEnabled()}
                        onChange={() => void setDictationEnabled(!dictationEnabled())}
                    />
                </SettingsRow>
                <Show when={dictationEnabled()}>
                    <SettingsRow
                        title={t("drift.voice.input.title")}
                        description={
                            selectedDeviceMissing()
                                ? t("drift.voice.input.missing")
                                : t("drift.voice.input.description")
                        }
                    >
                        <Picker
                            label={t("drift.voice.input.title")}
                            items={[
                                { id: "", label: t("drift.voice.input.systemDefault") },
                                ...audioInputDevices().map((device, index) => ({
                                    id: device.deviceId,
                                    label: device.label || t("drift.voice.input.unnamed", { index: index + 1 }),
                                })),
                            ]}
                            selected={selectedDeviceMissing() ? "" : (dictationInputDeviceId() ?? "")}
                            floating
                            bordered
                            chevronAtEnd
                            placement="below"
                            width={pickerWidth}
                            onPick={(value) => setDictationInputDeviceId(value || null)}
                        />
                    </SettingsRow>
                    <SettingsRow
                        title={t("drift.voice.provider.title")}
                        description={t("drift.voice.provider.description")}
                    >
                        <Picker
                            label={t("drift.voice.provider.title")}
                            items={dictationProviders
                                .filter((id) => id === "local" || keys.providers.some((provider) => provider.id === id))
                                .map((id) => ({ id, label: providerLabel(id) }))}
                            selected={dictationProvider()}
                            floating
                            bordered
                            chevronAtEnd
                            placement="below"
                            width={pickerWidth}
                            onPick={(value) => setDictationProvider(value as DictationProvider)}
                        />
                    </SettingsRow>
                    <Show when={cloud()}>{(provider) => <VoiceKeyRow provider={provider()} keys={keys} />}</Show>
                    <Show when={local()}>
                        <SettingsRow
                            title={t("drift.voice.model.title")}
                            description={t("drift.voice.model.description")}
                        >
                            <Picker
                                label={t("drift.voice.model.title")}
                                items={dictationModels.map((id) => ({
                                    id,
                                    label: t(modelLabels[id]),
                                    detail: formatBytes(modelInfo(id)?.bytes ?? 0),
                                }))}
                                selected={dictationModel()}
                                floating
                                bordered
                                chevronAtEnd
                                placement="below"
                                width={pickerWidth}
                                onPick={(value) => setDictationModel(value as DictationModel)}
                            />
                        </SettingsRow>
                        <SettingsRow
                            title={t("drift.voice.model.storage.title")}
                            description={storageDescription(selected())}
                        >
                            <Show
                                when={!downloading()}
                                fallback={
                                    <button
                                        class="h-8 rounded-md border border-edge px-3 text-xs text-ink-muted transition-colors hover:border-danger hover:text-danger"
                                        onClick={cancelVoiceModelDownload}
                                    >
                                        {t("common.cancel")}
                                    </button>
                                }
                            >
                                <Show
                                    when={selected()?.installed}
                                    fallback={
                                        <button
                                            class="h-8 rounded-md bg-accent px-3 text-xs font-medium text-accent-ink transition-opacity disabled:opacity-40"
                                            disabled={!!voiceModelBusy() || !voiceSupported()}
                                            onClick={() => void downloadVoiceModel(dictationModel())}
                                        >
                                            {t("drift.voice.model.download")}
                                        </button>
                                    }
                                >
                                    <button
                                        class="h-8 rounded-md border border-edge px-3 text-xs text-ink-muted transition-colors hover:border-danger hover:text-danger disabled:opacity-40"
                                        disabled={!!voiceModelBusy()}
                                        onClick={() => void removeVoiceModel(dictationModel())}
                                    >
                                        {t("drift.voice.model.remove")}
                                    </button>
                                </Show>
                            </Show>
                        </SettingsRow>
                        <SettingsRow
                            title={t("drift.voice.acceleration.title")}
                            description={
                                voiceAccelerated()
                                    ? t("drift.voice.acceleration.gpu")
                                    : t("drift.voice.acceleration.cpu")
                            }
                        >
                            <span
                                class="text-[0.75rem]"
                                classList={{ "text-ok": voiceAccelerated(), "text-ink-faint": !voiceAccelerated() }}
                            >
                                {voiceAccelerated()
                                    ? t("drift.voice.acceleration.on")
                                    : t("drift.voice.acceleration.off")}
                            </span>
                        </SettingsRow>
                    </Show>
                    <SettingsRow
                        title={t("drift.voice.dictation.language.title")}
                        description={t("drift.voice.dictation.language.description")}
                    >
                        <Picker
                            label={t("drift.voice.dictation.language.title")}
                            items={dictationLanguages.map((id) => ({
                                id,
                                label: languageLabels[id] || t("drift.voice.dictation.language.auto"),
                            }))}
                            selected={dictationLanguage()}
                            floating
                            bordered
                            chevronAtEnd
                            placement="below"
                            width={pickerWidth}
                            onPick={(value) => setDictationLanguage(value as DictationLanguage)}
                        />
                    </SettingsRow>
                    <SettingsRow
                        title={t("drift.voice.dictation.keyterms.title")}
                        description={t("drift.voice.dictation.keyterms.description")}
                    >
                        <input
                            aria-label={t("drift.voice.dictation.keyterms.title")}
                            class="h-8 w-full rounded-md border border-edge bg-raised/45 px-2.5 text-xs text-ink outline-none placeholder:text-ink-faint focus:border-accent sm:w-56"
                            placeholder={t("drift.voice.dictation.keyterms.placeholder")}
                            value={keyterms()}
                            onInput={(event) => setKeyterms(event.currentTarget.value)}
                            onChange={(event) => commitKeyterms(event.currentTarget.value)}
                        />
                    </SettingsRow>
                </Show>
            </SettingsGroup>

            <Show when={voiceProgress()}>
                {(value) => (
                    <div class="space-y-1.5 px-1">
                        <div class="flex items-center justify-between text-[0.72rem] text-ink-faint">
                            <span>{t("drift.voice.model.downloading")}</span>
                            <span class="font-mono">
                                {formatBytes(value().received)} / {formatBytes(value().total)}
                            </span>
                        </div>
                        <div class="h-1 overflow-hidden rounded-full bg-raised">
                            <div
                                class="h-full rounded-full bg-accent transition-all"
                                style={{ width: `${downloadPercent(value())}%` }}
                            />
                        </div>
                    </div>
                )}
            </Show>

            <Show when={!voiceSupported() && local()}>
                <div class="rounded-md border border-warn/30 bg-warn/10 px-2.5 py-2 text-xs text-warn">
                    {t("drift.voice.error.unsupported")}
                </div>
            </Show>
            <Show when={voiceModelError() || keys.error()}>
                <div class="px-1 text-xs text-danger">{voiceModelError() || keys.error()}</div>
            </Show>
            <Show when={dictationEnabled()}>
                <div class="px-1 text-[0.72rem] leading-relaxed text-ink-faint">
                    {cloud()
                        ? t("drift.voice.cloud.privacy", { provider: cloud()!.name })
                        : t("drift.voice.dictation.privacy")}
                </div>
            </Show>

            <ReadAloudGroup keys={keys} />
        </div>
    );
}
