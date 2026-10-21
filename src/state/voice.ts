import { persisted } from "./persist";

export const dictationLanguages = ["auto", "en", "es", "fr", "de", "hi", "it", "ja", "nl", "pt", "ru"] as const;
export type DictationLanguage = (typeof dictationLanguages)[number];

export const dictationModels = ["large-v3-turbo-q5_0", "small-q5_1", "base-q5_1"] as const;
export type DictationModel = (typeof dictationModels)[number];

export const maxKeyterms = 50;
const maxKeytermChars = 80;

/** `local` is whisper on this machine; the rest send each phrase to that provider through the engine. */
export const dictationProviders = ["local", "openai", "groq", "deepgram", "elevenlabs", "assemblyai"] as const;
export type DictationProvider = (typeof dictationProviders)[number];

/** `local` is the system's own offline voices; the rest send each piece of a reply to that provider. */
export const speechProviders = ["local", "openai", "elevenlabs"] as const;
export type SpeechProvider = (typeof speechProviders)[number];

/** OpenAI's built-in speech voices. */
export const openaiVoices = [
    "marin",
    "cedar",
    "alloy",
    "ash",
    "ballad",
    "coral",
    "echo",
    "fable",
    "nova",
    "onyx",
    "sage",
    "shimmer",
    "verse",
] as const;

// Off by default: dictation needs a model download, and most people never dictate.
export const [dictationEnabled, persistDictationEnabled] = persisted("drift.voice.dictation.enabled", false);
export const [dictationInputDeviceId, setDictationInputDeviceId] = persisted<string | null>(
    "drift.voice.dictation.inputDeviceId",
    null,
    normalizeInputDeviceId,
);
export const [dictationLanguage, setDictationLanguage] = persisted<DictationLanguage>(
    "drift.voice.dictation.language",
    "en",
    normalizeLanguage,
);
export const [dictationModel, setDictationModel] = persisted<DictationModel>(
    "drift.voice.dictation.model",
    "large-v3-turbo-q5_0",
    normalizeModel,
);
export const [dictationKeyterms, setDictationKeyterms] = persisted<string[]>(
    "drift.voice.dictation.keyterms",
    [],
    normalizeKeyterms,
);
// Local by default: a cloud provider is used only once picked here and given a key.
export const [dictationProvider, setDictationProvider] = persisted<DictationProvider>(
    "drift.voice.dictation.provider",
    "local",
    (value) => normalizeChoice(dictationProviders, value),
);
// Off by default: no play button and nothing spoken until turned on.
export const [readAloudEnabled, setReadAloudEnabled] = persisted(
    "drift.voice.readAloud.enabled",
    false,
    (value) => value === true,
);
export const [readAloudAuto, setReadAloudAuto] = persisted(
    "drift.voice.readAloud.auto",
    false,
    (value) => value === true,
);
export const [speechProvider, setSpeechProvider] = persisted<SpeechProvider>(
    "drift.voice.readAloud.provider",
    "local",
    (value) => normalizeChoice(speechProviders, value),
);
const [voices, setVoices] = persisted<Partial<Record<SpeechProvider, string>>>(
    "drift.voice.readAloud.voices",
    {},
    normalizeVoices,
);
/** The voice picked for each speech provider; a missing one is that provider's default. */
export const speechVoices = voices;

export function normalizeChoice<T extends string>(choices: readonly T[], value: unknown): T {
    return choices.includes(value as T) ? (value as T) : choices[0]!;
}

export function normalizeVoices(value: unknown): Partial<Record<SpeechProvider, string>> {
    if (!value || typeof value !== "object") return {};
    const voices: Partial<Record<SpeechProvider, string>> = {};
    for (const provider of speechProviders) {
        const voice = (value as Record<string, unknown>)[provider];
        if (typeof voice === "string" && voice.trim()) voices[provider] = voice.trim().slice(0, 200);
    }
    return voices;
}

export function setSpeechVoice(provider: SpeechProvider, voice: string) {
    const next = { ...speechVoices() };
    if (voice.trim()) next[provider] = voice.trim();
    else delete next[provider];
    setVoices(next);
}

export function normalizeLanguage(value: unknown): DictationLanguage {
    return dictationLanguages.includes(value as DictationLanguage) ? (value as DictationLanguage) : "en";
}

export function normalizeModel(value: unknown): DictationModel {
    return dictationModels.includes(value as DictationModel) ? (value as DictationModel) : "large-v3-turbo-q5_0";
}

export function normalizeInputDeviceId(value: unknown) {
    if (typeof value !== "string") return null;
    const deviceId = value.trim();
    return deviceId && deviceId !== "default" ? deviceId : null;
}

export function normalizeKeyterms(value: unknown) {
    if (!Array.isArray(value)) return [];
    return dedupeKeyterms(value.filter((item): item is string => typeof item === "string"));
}

export function parseKeyterms(text: string) {
    return dedupeKeyterms(text.split(/[,\n]/));
}

export function formatKeyterms(terms: string[]) {
    return terms.join(", ");
}

/** Whisper biases decoding toward an initial prompt, which is how custom vocabulary is honored. */
export function keytermPrompt(terms: string[]) {
    return terms.length ? `Vocabulary: ${terms.join(", ")}.` : "";
}

function dedupeKeyterms(values: string[]) {
    const seen = new Set<string>();
    const terms: string[] = [];
    for (const value of values) {
        const term = value.trim().slice(0, maxKeytermChars);
        const key = term.toLowerCase();
        if (!term || seen.has(key)) continue;
        seen.add(key);
        terms.push(term);
        if (terms.length === maxKeyterms) break;
    }
    return terms;
}
