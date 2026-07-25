import { expect, test } from "bun:test"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

test("settings expose the OpenCode language and sound catalogs", async () => {
  const { languages } = await import("../src/state/language")
  const { soundOptions } = await import("../src/ui/sounds")
  expect(languages.map((item) => item.id)).toEqual([
    "en",
    "zh",
    "zht",
    "ko",
    "de",
    "es",
    "fr",
    "da",
    "ja",
    "pl",
    "ru",
    "uk",
    "bs",
    "ar",
    "no",
    "br",
    "th",
    "tr",
  ])
  expect(soundOptions).toHaveLength(45)
  expect(new Set(soundOptions.map((item) => item.id)).size).toBe(45)
})

test("selected language dictionaries translate settings without loading every locale", async () => {
  const { loadDictionary, reasoningLevelLabel, t } = await import("../src/state/i18n")
  await loadDictionary("es")
  expect(t("settings.tab.general")).toBe("General")
  expect(t("settings.general.row.language.title")).toBe("Idioma")
  expect(t("common.reset")).toBe("Restablecer")
  expect(reasoningLevelLabel("xhigh")).toBe("Muy alto")
  expect(reasoningLevelLabel("custom")).toBe("Custom")
  await loadDictionary("en")
  expect(t("settings.general.row.language.title")).toBe("Language")
  expect(t("common.reset")).toBe("Reset")
})

test("prompt and agent editors are separate Server settings with inherited-value styling", async () => {
  const source = await Bun.file("src/ui/settings.tsx").text()
  expect(source).toContain('items: ["Providers", "MCP", "Prompts", "Agents"]')
  expect(source).toContain('<PromptEditorSection view="prompts" />')
  expect(source).toContain('<PromptEditorSection view="agents" />')
  expect(source).toContain('"text-ink-faint": !familyModified()')
  expect(source).toContain('"text-ink-faint": !agentPromptModified()')
  expect(source).toContain('"text-ink-faint": !agentBehaviorModified()')
  expect(source).toContain("disabled={props.disabled || !props.dirty}")
})

test("agent overrides retain only values changed from upstream", async () => {
  const { agentOverrideValue } = await import("../src/state/prompts")
  const inherited = { prompt: "Upstream", mode: "primary", tools: { bash: true, read: true } }
  expect(agentOverrideValue({ ...inherited, tools: { read: true, bash: true } }, inherited)).toEqual({})
  expect(agentOverrideValue({ ...inherited, mode: "subagent" }, inherited)).toEqual({ mode: "subagent" })
  expect(agentOverrideValue({ ...inherited, prompt: "Custom" }, inherited)).toEqual({ prompt: "Custom" })
  expect(
    agentOverrideValue(
      { prompt: "Custom", mode: "subagent" },
      { prompt: "Custom", mode: "primary" },
      { prompt: "Custom" },
    ),
  ).toEqual({ prompt: "Custom", mode: "subagent" })
})

test("Drift owns complete app-specific translations for every locale", async () => {
  const { languages } = await import("../src/state/language")
  const english = await import("../src/i18n/en")
  const keys = Object.keys(english.drift).sort()
  for (const language of languages) {
    const catalog = await import(`../src/i18n/${language.id}.ts`)
    expect(Object.keys(catalog.drift).sort()).toEqual(keys)
  }
  expect(await Bun.file("src/state/i18n.ts").text()).not.toContain("engine/upstream")
})

test("appearance exposes static presets plus custom theming", async () => {
  const { setCustomTheme, setTheme, lightTheme, themes } = await import("../src/state/theme")
  expect(themes).toHaveLength(9)
  setTheme("drift-paper")
  expect(lightTheme()).toBeTrue()
  setCustomTheme({ background: "#ffffff", surface: "#f5f5f5", text: "#111111", accent: "#3366cc" })
  setTheme("drift-custom")
  expect(lightTheme()).toBeTrue()
  setTheme("drift-dark")
})

test("settings elevation and toggle contrast follow their visual state", async () => {
  const [settings, toggles, styles] = await Promise.all([
    Bun.file("src/ui/settings.tsx").text(),
    Bun.file("src/ui/model-manager.tsx").text(),
    Bun.file("src/styles/app.css").text(),
  ])
  expect(settings).toContain('"settings-header-scrolled": contentScrolled()')
  expect(settings).toContain("setContentScrolled(event.currentTarget.scrollTop > 1)")
  expect(settings).toContain('class="flex min-w-0 flex-1 flex-col overflow-hidden"')
  expect(styles).toContain(".settings-header-scrolled::after")
  expect(styles).not.toContain(".settings-header::after")
  expect(toggles).toContain('"bg-ink-muted": !props.checked')
  expect(toggles).toContain('"translate-x-3 bg-accent-ink": props.checked')
})

test("appearance exposes persisted startup splash controls", async () => {
  const { splashDurations, splashExitAnimations, splashMascotAnimations } = await import("../src/state/startup")
  const settings = await Bun.file("src/ui/settings.tsx").text()
  expect(splashMascotAnimations).toEqual(["bounce", "float", "pulse", "still"])
  expect(splashExitAnimations).toEqual(["wave", "fade", "lift"])
  expect(splashDurations).toEqual([1500, 3200, 5000])
  expect(settings).toContain('title={t("startup.settings.title")}')
  expect(settings).toContain("setSplashEnabled(!splashEnabled())")
  expect(settings).toContain("value={splashFont()}")
})

test("code display defaults preserve source and diff structure", async () => {
  const {
    codeFontSize,
    codeTabWidth,
    codeWordWrap,
    diffIndicator,
    diffLineNumbers,
    diffWordWrap,
    syntaxThemePreset,
    syntaxThemePresets,
  } = await import("../src/state/code")
  expect(syntaxThemePresets).toEqual(["automatic", "github", "vitesse", "one", "dracula", "nord"])
  expect(syntaxThemePreset()).toBe("automatic")
  expect(codeFontSize()).toBe(13)
  expect(codeTabWidth()).toBe(4)
  expect(codeWordWrap()).toBeFalse()
  expect(diffWordWrap()).toBeFalse()
  expect(diffLineNumbers()).toBeTrue()
  expect(diffIndicator()).toBe("symbols")
  const { codeSettingOptions } = await import("../src/ui/settings")
  expect(codeSettingOptions()).toEqual({
    themes: ["automatic", "github", "vitesse", "one", "dracula", "nord"],
    fontSizes: [11, 12, 13, 14, 15, 16],
    tabWidths: [2, 4, 8],
    indicators: ["symbols", "bars", "background"],
  })
  const { codePreferenceBinding } = await import("../src/state/code")
  expect(codePreferenceBinding(13, 4, false, "automatic")).toEqual({
    size: "13px",
    tabSize: "4",
    wrap: "scroll",
    theme: "automatic",
  })
  expect(codePreferenceBinding(16, 8, true, "dracula").wrap).toBe("wrap")
})

test("notification migration and global auto-accept stay explicit", async () => {
  const { autoAcceptAllowed, notificationDefaults, soundDefaults } = await import("../src/state/prefs")
  expect(notificationDefaults(true)).toEqual({ agent: true, permission: true, error: true })
  expect(soundDefaults()).toEqual({ agent: "none", permission: "none", error: "none" })
  expect(autoAcceptAllowed(true, [], "child")).toBeTrue()
  expect(autoAcceptAllowed(false, ["thread"], "thread")).toBeTrue()
  expect(autoAcceptAllowed(false, ["parent"], "child", "parent")).toBeTrue()
  expect(autoAcceptAllowed(false, ["linked"], "child", undefined, "linked")).toBeTrue()
  expect(autoAcceptAllowed(false, ["other"], "child", "parent", "linked")).toBeFalse()
})
