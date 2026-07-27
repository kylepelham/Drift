import { createSignal } from "solid-js"
import { dict, drift } from "../i18n/en"
import { createLatestOnly } from "./latest"

type Dictionary = Record<string, string>
const english = { ...dict, ...drift } as Dictionary
const reasoningLevels: Record<string, string> = {
  none: "drift.reasoning.level.none",
  minimal: "drift.reasoning.level.minimal",
  low: "drift.reasoning.level.low",
  medium: "drift.reasoning.level.medium",
  high: "drift.reasoning.level.high",
  xhigh: "drift.reasoning.level.xhigh",
  max: "drift.reasoning.level.max",
}

const loaders: Record<string, () => Promise<{ dict: Dictionary; drift: Dictionary }>> = {
  ar: () => import("../i18n/ar"),
  br: () => import("../i18n/br"),
  bs: () => import("../i18n/bs"),
  da: () => import("../i18n/da"),
  de: () => import("../i18n/de"),
  es: () => import("../i18n/es"),
  fr: () => import("../i18n/fr"),
  ja: () => import("../i18n/ja"),
  ko: () => import("../i18n/ko"),
  no: () => import("../i18n/no"),
  pl: () => import("../i18n/pl"),
  ru: () => import("../i18n/ru"),
  th: () => import("../i18n/th"),
  tr: () => import("../i18n/tr"),
  uk: () => import("../i18n/uk"),
  zh: () => import("../i18n/zh"),
  zht: () => import("../i18n/zht"),
}

const [dictionary, setDictionary] = createSignal(english)
const cache = new Map<string, Dictionary>([["en", english]])
const load = createLatestOnly()

export async function loadDictionary(language: string) {
  const token = load.begin()
  const cached = cache.get(language)
  setDictionary(cached ?? english)
  if (cached) return
  const loader = loaders[language]
  if (!loader) return
  const module = await loader().catch(() => null)
  if (!module) return
  const next = { ...english, ...module.dict, ...module.drift }
  cache.set(language, next)
  // A slow chunk for a language the user already switched away from must not win.
  if (load.isCurrent(token)) setDictionary(next)
}

export function t(key: string, variables?: Record<string, string | number>) {
  const value = dictionary()[key] ?? english[key] ?? key
  if (!variables) return value
  return Object.entries(variables).reduce((text, [name, replacement]) => text.replaceAll(`{{${name}}}`, String(replacement)), value)
}

export function agentLabel(name: string) {
  const normalized = name.toLowerCase()
  if (["build", "plan", "general", "explore"].includes(normalized)) return t(`drift.agent.${normalized}`)
  return name.charAt(0).toUpperCase() + name.slice(1)
}

export function reasoningLevelLabel(name: string) {
  const key = reasoningLevels[name.toLowerCase()]
  return key ? t(key) : name.charAt(0).toUpperCase() + name.slice(1)
}
