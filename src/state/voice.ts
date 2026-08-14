import { persisted } from "./persist"

export const dictationLanguages = ["auto", "en", "es", "fr", "de", "hi", "it", "ja", "nl", "pt", "ru"] as const
export type DictationLanguage = (typeof dictationLanguages)[number]

export const dictationModels = ["large-v3-turbo-q5_0", "small-q5_1", "base-q5_1"] as const
export type DictationModel = (typeof dictationModels)[number]

export const maxKeyterms = 50
const maxKeytermChars = 80

// Off by default: dictation needs a model download, and most people never dictate.
export const [dictationEnabled, persistDictationEnabled] = persisted("drift.voice.dictation.enabled", false)
export const [dictationInputDeviceId, setDictationInputDeviceId] = persisted<string | null>(
  "drift.voice.dictation.inputDeviceId",
  null,
  normalizeInputDeviceId,
)
export const [dictationLanguage, setDictationLanguage] = persisted<DictationLanguage>(
  "drift.voice.dictation.language",
  "en",
  normalizeLanguage,
)
export const [dictationModel, setDictationModel] = persisted<DictationModel>(
  "drift.voice.dictation.model",
  "large-v3-turbo-q5_0",
  normalizeModel,
)
export const [dictationKeyterms, setDictationKeyterms] = persisted<string[]>(
  "drift.voice.dictation.keyterms",
  [],
  normalizeKeyterms,
)

export function normalizeLanguage(value: unknown): DictationLanguage {
  return dictationLanguages.includes(value as DictationLanguage) ? (value as DictationLanguage) : "en"
}

export function normalizeModel(value: unknown): DictationModel {
  return dictationModels.includes(value as DictationModel) ? (value as DictationModel) : "large-v3-turbo-q5_0"
}

export function normalizeInputDeviceId(value: unknown) {
  if (typeof value !== "string") return null
  const deviceId = value.trim()
  return deviceId && deviceId !== "default" ? deviceId : null
}

export function normalizeKeyterms(value: unknown) {
  if (!Array.isArray(value)) return []
  return dedupeKeyterms(value.filter((item): item is string => typeof item === "string"))
}

export function parseKeyterms(text: string) {
  return dedupeKeyterms(text.split(/[,\n]/))
}

export function formatKeyterms(terms: string[]) {
  return terms.join(", ")
}

/** Whisper biases decoding toward an initial prompt, which is how custom vocabulary is honored. */
export function keytermPrompt(terms: string[]) {
  return terms.length ? `Vocabulary: ${terms.join(", ")}.` : ""
}

function dedupeKeyterms(values: string[]) {
  const seen = new Set<string>()
  const terms: string[] = []
  for (const value of values) {
    const term = value.trim().slice(0, maxKeytermChars)
    const key = term.toLowerCase()
    if (!term || seen.has(key)) continue
    seen.add(key)
    terms.push(term)
    if (terms.length === maxKeyterms) break
  }
  return terms
}
