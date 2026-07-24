import { createEffect, createSignal, onCleanup } from "solid-js"
import { persisted } from "./persist"

export const themes = [
  "drift-dark",
  "drift-graphite",
  "drift-midnight",
  "drift-slate",
  "drift-forest",
  "drift-aubergine",
  "drift-light",
  "drift-paper",
  "drift-custom",
] as const
export type ThemeName = (typeof themes)[number]
export type CustomTheme = { background: string; surface: string; text: string; accent: string }

export const [theme, setTheme] = persisted<ThemeName>("drift.theme", "drift-dark")
export const [uiFont, setUiFont] = persisted("drift.theme.uiFont", "")
export const [codeFont, setCodeFont] = persisted("drift.theme.codeFont", "")
export const [customTheme, setCustomTheme] = persisted<CustomTheme>("drift.theme.custom", {
  background: "#111318",
  surface: "#1b1e25",
  text: "#e8eaf0",
  accent: "#a78bfa",
})
const savedCustomCss = localStorage.getItem("drift.theme.customCss")
export const [customCss, setCustomCssValue] = createSignal<string>(savedCustomCss ? JSON.parse(savedCustomCss) : "")
let cssPersistTimer: ReturnType<typeof setTimeout> | undefined

export function setCustomCss(value: string) {
  const next = value.slice(0, 20_000)
  setCustomCssValue(next)
  clearTimeout(cssPersistTimer)
  cssPersistTimer = setTimeout(() => localStorage.setItem("drift.theme.customCss", JSON.stringify(next)), 200)
}

export function setCustomThemeColor(color: keyof CustomTheme, value: string) {
  setCustomTheme({ ...customTheme(), [color]: value })
}

export function lightTheme() {
  if (theme() === "drift-light" || theme() === "drift-paper") return true
  if (theme() !== "drift-custom") return false
  const match = customTheme().background.match(/^#([\da-f]{2})([\da-f]{2})([\da-f]{2})$/i)
  if (!match) return false
  return (Number.parseInt(match[1], 16) * 299 + Number.parseInt(match[2], 16) * 587 + Number.parseInt(match[3], 16) * 114) / 255_000 > 0.6
}

export function bindTheme() {
  createEffect(() => {
    document.documentElement.dataset.theme = theme()
  })
  createEffect(() => {
    document.documentElement.style.setProperty("--ui-font", uiFont().trim() || '"Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif')
    document.documentElement.style.setProperty("--code-font", codeFont().trim() || '"Cascadia Code", Consolas, ui-monospace, monospace')
  })
  createEffect(() => {
    const colors = customTheme()
    document.documentElement.style.setProperty("--custom-bg", colors.background)
    document.documentElement.style.setProperty("--custom-surface", colors.surface)
    document.documentElement.style.setProperty("--custom-ink", colors.text)
    document.documentElement.style.setProperty("--custom-accent", colors.accent)
  })
  createEffect(() => {
    const value = customCss()
    const timer = setTimeout(() => {
      const style = document.querySelector<HTMLStyleElement>("#drift-custom-css") ?? document.createElement("style")
      style.id = "drift-custom-css"
      style.textContent = value
      if (!style.isConnected) document.head.append(style)
    }, 75)
    onCleanup(() => clearTimeout(timer))
  })
}
