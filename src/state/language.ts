import { createEffect } from "solid-js"
import { loadDictionary } from "./i18n"
import { persisted } from "./persist"

export const languages = [
  { id: "en", label: "English", locale: "en" },
  { id: "zh", label: "简体中文", locale: "zh-CN" },
  { id: "zht", label: "繁體中文", locale: "zh-TW" },
  { id: "ko", label: "한국어", locale: "ko" },
  { id: "de", label: "Deutsch", locale: "de" },
  { id: "es", label: "Español", locale: "es" },
  { id: "fr", label: "Français", locale: "fr" },
  { id: "da", label: "Dansk", locale: "da" },
  { id: "ja", label: "日本語", locale: "ja" },
  { id: "pl", label: "Polski", locale: "pl" },
  { id: "ru", label: "Русский", locale: "ru" },
  { id: "uk", label: "Українська", locale: "uk" },
  { id: "bs", label: "Bosanski", locale: "bs" },
  { id: "ar", label: "العربية", locale: "ar", direction: "rtl" },
  { id: "no", label: "Norsk", locale: "no" },
  { id: "br", label: "Português (Brasil)", locale: "pt-BR" },
  { id: "th", label: "ไทย", locale: "th" },
  { id: "tr", label: "Türkçe", locale: "tr" },
] as const

export type LanguageId = (typeof languages)[number]["id"]
export const [language, setLanguage] = persisted<LanguageId>("drift.language", "en")

export function bindLanguage() {
  createEffect(() => {
    const selected = languages.find((item) => item.id === language()) ?? languages[0]
    document.documentElement.lang = selected.locale
    document.documentElement.dir = "direction" in selected ? selected.direction : "ltr"
    void loadDictionary(selected.id)
  })
}
