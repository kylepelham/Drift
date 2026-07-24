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
  expect(reasoningLevelLabel("xhigh")).toBe("Muy alto")
  expect(reasoningLevelLabel("custom")).toBe("Custom")
  await loadDictionary("en")
  expect(t("settings.general.row.language.title")).toBe("Language")
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
