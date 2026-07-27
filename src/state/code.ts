import { createEffect } from "solid-js"
import { persisted } from "./persist"
import { lightTheme } from "./theme"

export const syntaxThemePresets = ["automatic", "github", "vitesse", "one", "dracula", "nord"] as const
export type SyntaxThemePreset = (typeof syntaxThemePresets)[number]

export const codeFontSizes = [11, 12, 13, 14, 15, 16] as const
export const codeTabWidths = [2, 4, 8] as const

export const diffIndicators = ["symbols", "bars", "background"] as const
export type DiffIndicator = (typeof diffIndicators)[number]

export const [syntaxThemePreset, setSyntaxThemePreset] = persisted<SyntaxThemePreset>(
  "drift.code.syntaxTheme",
  "automatic",
)
export const [codeFontSize, setCodeFontSize] = persisted("drift.code.fontSize", 13)
export const [codeTabWidth, setCodeTabWidth] = persisted("drift.code.tabWidth", 4)
export const [codeWordWrap, setCodeWordWrap] = persisted("drift.code.wordWrap", false)
export const [diffWordWrap, setDiffWordWrap] = persisted("drift.code.diffWordWrap", false)
export const [diffLineNumbers, setDiffLineNumbers] = persisted("drift.code.diffLineNumbers", true)
export const [diffIndicator, setDiffIndicator] = persisted<DiffIndicator>("drift.code.diffIndicator", "background")

export function syntaxTheme() {
  const preset = syntaxThemePreset()
  const light = lightTheme()
  if (preset === "automatic" || preset === "github") return light ? "github-light" : "github-dark-default"
  if (preset === "vitesse") return light ? "vitesse-light" : "vitesse-dark"
  if (preset === "one") return light ? "one-light" : "one-dark-pro"
  return preset
}

export function codePreferenceBinding(
  fontSize: number,
  tabWidth: number,
  wrap: boolean,
  themePreset: SyntaxThemePreset,
) {
  return {
    size: `${fontSize}px`,
    tabSize: `${tabWidth}`,
    wrap: wrap ? "wrap" : "scroll",
    theme: themePreset,
  }
}

export function bindCodePreferences() {
  createEffect(() => {
    const binding = codePreferenceBinding(codeFontSize(), codeTabWidth(), codeWordWrap(), syntaxThemePreset())
    const root = document.documentElement
    root.style.setProperty("--code-size", binding.size)
    root.style.setProperty("--code-tab-size", binding.tabSize)
    root.dataset.codeWrap = binding.wrap
    root.dataset.syntaxTheme = binding.theme
  })
}
