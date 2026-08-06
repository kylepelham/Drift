import { expect, test } from "bun:test"

const settingsStorage = new Map<string, string>()
if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: {
      getItem: (key: string) => settingsStorage.get(key) ?? null,
      setItem: (key: string, value: string) => settingsStorage.set(key, value),
    },
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

test("LM Studio readiness uses the loaded context required by the coding agent", async () => {
  const { formatModelContext, lmStudioMinimumContext, lmStudioModelReady } = await import("../src/state/lm-studio")
  const model = {
    capabilities: { toolcall: true },
    limit: { context: 4096 },
  }
  expect(lmStudioMinimumContext).toBe(32768)
  expect(formatModelContext(4096)).toBe("4K")
  expect(formatModelContext(32768)).toBe("32K")
  expect(lmStudioModelReady(model as never)).toBe(false)
  expect(lmStudioModelReady({ ...model, limit: { context: 32768 } } as never)).toBe(true)
  expect(lmStudioModelReady({ ...model, capabilities: { toolcall: false }, limit: { context: 65536 } } as never)).toBe(false)
})

test("selected language dictionaries translate settings without loading every locale", async () => {
  const { loadDictionary, reasoningLevelLabel, t } = await import("../src/state/i18n")
  await loadDictionary("es")
  expect(t("settings.tab.general")).toBe("General")
  expect(t("settings.general.row.language.title")).toBe("Idioma")
  expect(t("common.reset")).toBe("Restablecer")
  expect(t("drift.remote.title")).toBe("Remote Access")
  expect(t("drift.settings.prompts")).toBe("Prompts")
  expect(t("drift.slash.spawn.review")).toBe("Review")
  expect(t("drift.attachment.kind.pdf")).toBe("PDF")
  expect(reasoningLevelLabel("xhigh")).toBe("Muy alto")
  expect(reasoningLevelLabel("custom")).toBe("Custom")
  await loadDictionary("en")
  expect(t("settings.general.row.language.title")).toBe("Language")
  expect(t("common.reset")).toBe("Reset")
})

test("prompt and agent editors are separate Server settings with inherited-value styling", async () => {
  const source = await Bun.file("src/ui/settings.tsx").text()
  expect(source).toContain('items: ["Tools", "Providers", "MCP", "Prompts", "Agents"]')
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

const pendingKeys = (prefix: string, suffixes: string) =>
  suffixes
    .trim()
    .split(/\s+/)
    .map((suffix) => `${prefix}.${suffix}`)

/** Keys that deliberately fall back to English until locale-specific translations ship. */
const pendingTranslation = new Set([
  "drift.mobile.openNavigation",
  "drift.settings.code",
  ...pendingKeys(
    "drift.code",
    `
      syntax layout diffs spaces
      syntaxTheme.title syntaxTheme.description
      theme.automatic theme.github theme.vitesse theme.one theme.dracula theme.nord
      fontSize.title fontSize.description tabWidth.title tabWidth.description
      wordWrap.title wordWrap.description diffWordWrap.title diffWordWrap.description
      lineNumbers.title lineNumbers.description diffIndicator.title diffIndicator.description
      diffIndicators.symbols diffIndicators.bars diffIndicators.background
    `,
  ),
  ...pendingKeys("drift.engine", "restart restarting stopped.title"),
  ...pendingKeys(
    "drift.lmStudio",
    `
      apiToken contextTooSmall description discovered modelReady noReady notLoaded ready refresh
      refreshed refreshFailed unavailable
    `,
  ),
  ...pendingKeys(
    "drift.mcp",
    `
      servers registry add edit remove confirmRemove approve reject revoke authenticate pendingApproval
      invalidStatus rejectedStatus awaitingReport selectWorkspace saved removed approved rejected revoked
      name nameRequired registrySearch registrySource registryLoadFailed registryUnavailable install
      installedLabel installed
      form.nameInvalid form.type form.local form.remote form.enabled form.timeout form.command
      form.executable form.argument form.addArgument form.removeArgument form.cwd form.environment
      form.url form.headers form.oauth form.oauth.auto form.oauth.disabled form.oauth.configured
      form.clientId form.clientSecret form.scope form.callbackPort form.redirectUri form.key form.value
      form.addPair form.removePair form.commandRequired form.urlRequired form.urlInvalid
      form.timeoutInvalid form.pairInvalid form.callbackPortInvalid form.redirectUriInvalid
      toast.pending.title toast.pending.message toast.exact toast.openSettings toast.failed
    `,
  ),
  ...pendingKeys(
    "drift.recovery",
    `
      chooseModel continue durableHint explanation failedModel notification.body notification.title
      open starting subagent title
    `,
  ),
  ...pendingKeys(
    "drift.remote",
    `
      address connected connectionUrl copied copy deckHelp enable
      enableDescription gateway listening manageOnDesktop noLanAddress rotate rotated
      securityWarning title
    `,
  ),
  ...pendingKeys(
    "drift.settings.prompts",
    `
      agentDescription agentPrompt agents behavior familyDescription inheritsFamily invalidJson
      modelFamilies restart saveBeforeSwitch systemPrompt upstreamOriginal
    `,
  ),
  "drift.settings.prompts",
  ...pendingKeys(
    "drift.slash",
    `
      fork fork.active fork.active.description fork.all fork.all.description fork.invalid
      spawn spawn.implement spawn.implement.description spawn.investigate spawn.investigate.description
      spawn.required spawn.review spawn.review.description
    `,
  ),
  ...pendingKeys(
    "drift.storage",
    `
      actions analyze analyze.action analyze.description analyzing auto auto.description cleanup compact
      compact.action compact.description compacting estimated free prune prune.action prune.available
      prune.description pruning refresh subtitle
      rule.archived rule.archived.description rule.orphan rule.orphan.description rule.subagent
      rule.subagent.description rule.superseded rule.superseded.description
      sessions sessions.archived sessions.archived.description sessions.subagent
      sessions.subagent.description sessions.total sessions.total.description
      table.event table.event.hint table.message table.message.hint table.part table.part.hint
    `,
  ),
  "drift.storage",
  ...pendingKeys(
    "drift.voice",
    `
      acceleration.cpu acceleration.gpu acceleration.off acceleration.on acceleration.title dictation
      dictation.enabled.description dictation.enabled.title dictation.keyterms.description
      dictation.keyterms.placeholder dictation.keyterms.title dictation.language.auto
      dictation.language.description dictation.language.title dictation.privacy error.download
      error.microphone error.noMicrophone error.permission error.unsupported listening model.balanced
      model.best model.description model.download model.downloading model.fastest model.remove
      model.storage.missing model.storage.ready model.storage.title model.title start starting stop transcribing
    `,
  ),
  "drift.voice",
])

/** Values that intentionally remain identical in every language. */
const invariantTranslation = new Set([
  "drift.about.version",
  "drift.attachment.kind.pdf",
  "drift.notification.threadError",
  "drift.settings.section",
])

type Catalog = { dict: Record<string, string>; drift: Record<string, string> }
const featureTranslations = (catalog: Catalog) =>
  Object.fromEntries(
    Object.entries({ ...catalog.dict, ...catalog.drift }).filter(([key]) => key.startsWith("drift.")),
  ) as Record<string, string>

test("Drift owns explicit app-specific translations for every locale", async () => {
  const { languages } = await import("../src/state/language")
  const english = featureTranslations(await import("../src/i18n/en"))
  const nonEnglish = languages.filter((language) => language.id !== "en")
  const localized = await Promise.all(
    nonEnglish.map(async (language) => featureTranslations(await import(`../src/i18n/${language.id}.ts`))),
  )
  const translated = (keys: string[]) =>
    keys.filter((key) => !pendingTranslation.has(key) && !invariantTranslation.has(key)).sort()
  const keys = translated(Object.keys(english))

  for (const catalog of localized) expect(translated(Object.keys(catalog))).toEqual(keys)

  const unknown = [...pendingTranslation].filter((key) => !(key in english))
  const stale = [...pendingTranslation].filter((key) => localized.every((catalog) => key in catalog))
  const unknownInvariant = [...invariantTranslation].filter((key) => !(key in english))
  const copiedInvariant = [...invariantTranslation].filter((key) => localized.some((catalog) => key in catalog))
  const copiedEnglish = keys.filter((key) => localized.every((catalog) => catalog[key] === english[key]))

  expect(unknown, "pendingTranslation names keys that do not exist in en.ts").toEqual([])
  expect(stale, "pendingTranslation names keys that are translated now; remove them").toEqual([])
  expect(unknownInvariant, "invariantTranslation names keys that do not exist in en.ts").toEqual([])
  expect(copiedInvariant, "invariant translations must use the English fallback").toEqual([])
  expect(copiedEnglish, "English copies must be pending fallbacks or explicit invariants").toEqual([])

  for (const language of nonEnglish) {
    const source = await Bun.file(`src/i18n/${language.id}.ts`).text()
    expect(source).not.toMatch(/^import\s/m)
    expect(source).not.toMatch(/\.\.\.[A-Za-z_$]/)
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
    Bun.file("src/ui/controls.tsx").text(),
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
  const { splashDuration, splashDurations, splashExitAnimation, splashExitAnimations, splashMascotAnimations } =
    await import("../src/state/startup")
  const settings = await Bun.file("src/ui/settings.tsx").text()
  expect(splashMascotAnimations).toEqual(["bounce", "float", "pulse", "still"])
  expect(splashExitAnimations).toEqual(["wave", "fade", "lift"])
  expect(splashDurations).toEqual([1500, 3200, 5000])
  expect(splashExitAnimation()).toBe("fade")
  expect(splashDuration()).toBe(3200)
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
  expect(diffIndicator()).toBe("background")
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

test("shell timeout preferences normalize and persist explicit no-timeout", async () => {
  const { normalizeShellTimeout, setShellTimeoutMs, shellTimeoutMs, shellTimeoutPresets } = await import(
    "../src/state/prefs"
  )
  expect(shellTimeoutPresets).toEqual([60_000, 300_000, 900_000, 1_800_000])
  expect(normalizeShellTimeout(null)).toBeNull()
  expect(normalizeShellTimeout(60_000)).toBe(60_000)
  expect(normalizeShellTimeout(59_999)).toBeNull()
  expect(normalizeShellTimeout(86_400_001)).toBeNull()
  expect(normalizeShellTimeout("300000")).toBeNull()

  const setItem = localStorage.setItem
  const writes = new Map<string, string>()
  localStorage.setItem = (key, value) => writes.set(key, value)
  try {
    setShellTimeoutMs(900_000)
    expect(shellTimeoutMs()).toBe(900_000)
    expect(writes.get("drift.shell.timeout")).toBe("900000")
    setShellTimeoutMs(null)
    expect(shellTimeoutMs()).toBeNull()
    expect(writes.get("drift.shell.timeout")).toBe("null")
  } finally {
    localStorage.setItem = setItem
  }
})

test("the About mascot always disposes its scene, including when it loads after unmount", async () => {
  const { mountScene } = await import("../src/ui/jellyfish")
  const settle = async () => {
    for (let tick = 0; tick < 4; tick++) await Promise.resolve()
  }

  let disposed = 0
  let land!: (dispose: () => void) => void
  const slow = new Promise<() => void>((resolve) => (land = resolve))
  mountScene(() => slow, () => {})()
  land(() => disposed++)
  await settle()
  expect(disposed).toBe(1)

  let live = 0
  const cleanup = mountScene(() => Promise.resolve(() => live++), () => {})
  await settle()
  expect(live).toBe(0)
  cleanup()
  cleanup()
  expect(live).toBe(1)

  let fallbacks = 0
  mountScene(() => Promise.reject(new Error("no webgl")), () => fallbacks++)
  mountScene(() => Promise.resolve(undefined), () => fallbacks++)
  await settle()
  expect(fallbacks).toBe(2)

  let ignored = 0
  mountScene(() => Promise.reject(new Error("no webgl")), () => ignored++)()
  await settle()
  expect(ignored).toBe(0)
})
