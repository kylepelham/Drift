import "./source"
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
  expect(lmStudioModelReady({ ...model, capabilities: { toolcall: false }, limit: { context: 65536 } } as never)).toBe(
    false,
  )
})

test("selected language dictionaries translate settings without loading every locale", async () => {
  const { loadDictionary, reasoningLevelLabel, t } = await import("../src/state/i18n")
  await loadDictionary("es")
  expect(t("settings.tab.general")).toBe("General")
  expect(t("settings.general.row.language.title")).toBe("Idioma")
  expect(t("common.reset")).toBe("Restablecer")
  expect(t("drift.remote.title")).toBe("Remote Access")
  expect(t("drift.settings.prompts")).toBe("Prompts")
  expect(t("drift.slash.spawn.required")).toBe("Say what the new thread should do after /spawn.")
  expect(t("drift.attachment.kind.pdf")).toBe("PDF")
  expect(reasoningLevelLabel("xhigh")).toBe("Muy alto")
  expect(reasoningLevelLabel("custom")).toBe("Custom")
  await loadDictionary("en")
  expect(t("settings.general.row.language.title")).toBe("Language")
  expect(t("common.reset")).toBe("Reset")
})

test("base prompts and agents are one Server setting, with inherited values styled apart and save only for changes", async () => {
  const source = await Bun.file("src/ui/settings.tsx").text()
  const editor = await Bun.file("src/ui/settings-prompts.tsx").text()
  expect(source).toContain(
    'items: ["Tools", "Providers", "Usage", "Skills", "MCP", "Plugins", "Prompts", "Permissions"]',
  )
  expect(source).toContain("<PromptsSection />")
  expect(await Bun.file("src/ui/settings-base-prompts.tsx").exists()).toBeFalse()
  expect(editor).toContain('"text-ink-faint": !props.customized && !changed()')
  expect(editor).toContain("disabled={props.saving || !props.dirty}")
})

test("model-family base prompts are edited and reset in the engine, never through the shell's family overrides", async () => {
  const settings = await Bun.file("src/ui/settings.tsx").text()
  const editor = await Bun.file("src/ui/settings-prompts.tsx").text()
  expect(editor).toContainCode("const content = baseDraft(id)")
  expect(editor).toContainCode("engine.actions.saveBasePrompt(id, content)")
  expect(editor).toContainCode("engine.actions.resetBasePrompt(id)")
  expect(editor).not.toContain("readOnly")
  expect(settings).not.toContain("`family:")
  expect(settings).not.toContain("familyUnavailable")
})

test("settings search covers every category and finds feature descriptions", async () => {
  const { loadDictionary } = await import("../src/state/i18n")
  const { settingsSearchResults } = await import("../src/ui/settings")
  await loadDictionary("en")

  const categories = [
    "General",
    "Appearance",
    "Code",
    "Notifications",
    "Voice",
    "Shortcuts",
    "Tools",
    "Providers",
    "Skills",
    "MCP",
    "Plugins",
    "Prompts",
    "Storage",
    "Remote Access",
    "About",
  ] as const
  for (const category of categories) {
    expect(
      settingsSearchResults(category).some((item) => item.section === category),
      category,
    ).toBeTrue()
  }

  expect(settingsSearchResults("shell commands child processes")[0]?.section).toBe("Tools")
  expect(settingsSearchResults("compact database")[0]?.section).toBe("Storage")
  expect(settingsSearchResults("engine version")[0]?.section).toBe("About")
})

test("Settings offers no Jev tool routing: the native engine has none, so a toggle would configure nothing", async () => {
  const source = await Bun.file("src/ui/settings.tsx").text()
  expect(source).not.toContain("ToolRoutingSetting")
  expect(source).not.toContain("drift.settings.toolRouting")
  expect(source).not.toContain('t("drift.settings.shellTimeout.scope")')
  expect(await Bun.file("src/ui/settings-tool-routing.tsx").exists()).toBeFalse()
  expect(await Bun.file("src/state/tool-routing.ts").exists()).toBeFalse()
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

test("agent overrides saved or reset reach the engine for desktop and companion callers", async () => {
  const prompts = await Bun.file("src-tauri/src/prompts.rs").text()
  const remote = await Bun.file("src-tauri/src/remote.rs").text()
  for (const command of ["prompt_save", "prompt_reset"]) {
    const body = prompts.slice(prompts.indexOf(`pub(crate) fn ${command}(`)).split("\n}")[0]!
    expect(body).toContain("app: AppHandle")
    expect(body.trimEnd().endsWith("crate::native::push_agent_overrides(&app, &store)")).toBeTrue()
    expect(remote).toContain(`prompts::${command}(`)
  }
  const ui = await Bun.file("src/ui/settings-prompts.tsx").text()
  expect(ui).toContain("await write()\n      await engine.actions.refreshAgents()")
  expect(ui).toContain('t("drift.settings.prompts.saved")')
  expect(ui).not.toContain("showRestartNotice")
})

const pendingKeys = (prefix: string, suffixes: string) =>
  suffixes
    .trim()
    .split(/\s+/)
    .map((suffix) => `${prefix}.${suffix}`)

/** Keys that deliberately fall back to English until locale-specific translations ship. */
const pendingTranslation = new Set([
  ...pendingKeys("drift.thread", "openSubagent"),
  ...pendingKeys("drift.settings.autoCompact", "title description"),
  ...pendingKeys("drift.settings.prompts", "behaviorRefused"),
  "drift.message.forkHere",
  ...pendingKeys("drift.about", "row.native.title row.native.description native.connected native.offline"),
  "drift.markdown.linkFailed",
  ...pendingKeys("drift.context", "window systemAndTools user assistant tool"),
  ...pendingKeys(
    "drift.usage",
    `
      title settingsDescription none refresh session weekly weeklyModel monthly period premium chat resetsInMinutes resetsInHours resetsInDays resetsAt
      resetsSoon loading expired unsubscribed failed empty
    `,
  ),
  "drift.tool.readThread",
  "drift.mobile.openNavigation",
  "drift.settings.agents.automaticSmallModel",
  "drift.settings.agents.currentSessionModel",
  "drift.settings.code",
  ...pendingKeys("drift.settings.search", "empty placeholder"),
  ...pendingKeys("drift.chat.retry", "switchModel switchingModel"),
  ...pendingKeys("drift.chat.spawned", "copy instruction"),
  ...pendingKeys("drift.model", "smallContext unknownContext"),
  ...pendingKeys("drift.mcp.transport", "stdio streamable_http sse"),
  ...pendingKeys("drift.mcp.era", "stateless legacy"),
  ...pendingKeys("drift.mcp.form", "transport cwd cwdDefault timeout timeoutNone timeoutInvalid"),
  ...pendingKeys("drift.mcp.form", "app appHint clientId clientSecret clientSecretNone scopes scopesNone"),
  ...pendingKeys("drift.mcp", "signIn signOut signInOpened status.needsSignIn"),
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
      servers registry add edit engineDescription status.disconnected removed
      name nameRequired registrySearch registrySource registryLoadFailed install
      registry.filter registry.filter.all registry.filter.remote registry.filter.local registry.more registry.official registry.searchingOfficial
      registry.empty registry.back registry.repository registry.website registry.runAs registry.required
      registry.optional registry.secret registry.secretNote registry.installing registry.stars registry.needsKey
      registry.note.remote registry.note.docker registry.note.latest registry.note.local
      installedLabel installed
      form.nameInvalid form.type form.local form.remote form.command
      form.executable form.argument form.addArgument form.removeArgument form.environment
      form.url form.headers form.key form.value form.savedValue
      form.addPair form.removePair form.commandRequired form.urlRequired form.urlInvalid form.pairInvalid
    `,
  ),
  ...pendingKeys(
    "drift.remote",
    `
      connected copied copy enable enableDescription manageOnDesktop noLanAddress title
      connect.title connect.open connect.warning connect.code
      link.action link.linked link.waiting
      devices.title devices.empty devices.link devices.password devices.lastSeen devices.revoke devices.revokeAll
      password.title password.description password.on password.off password.setUp password.change
      password.turnOff password.username password.password password.confirm password.mismatch
      password.save password.cancel password.note
      certificate.title certificate.description encryption.fingerprint
      device.title device.signedIn device.signOut toast.title toast.message toast.open
    `,
  ),
  ...pendingKeys(
    "drift.search",
    `
      archived clear close empty mode.content mode.name next previous
      sessions.placeholder transcript transcript.placeholder
    `,
  ),
  ...pendingKeys("drift.settings.prompts", "agentPrompt familyDescription inheritsFamily saved systemPrompt"),
  "drift.shortcuts.findInSession",
  ...pendingKeys(
    "drift.slash",
    `
      fork fork.active fork.active.description fork.all fork.all.description fork.invalid
      spawn spawn.required
    `,
  ),
  ...pendingKeys(
    "drift.storage",
    `
      actions compact compact.action compact.description compacting estimated free prune prune.action
      prune.description pruning refresh subtitle
      sessions sessions.archived sessions.archived.description sessions.subagent
      sessions.subagent.description sessions.total sessions.total.description
      table.part table.part.hint table.blob table.blob.hint table.undo table.undo.hint table.output table.output.hint
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
  "drift.plugins.fieldType.json",
  "drift.registry.sources.kind.url",
  "drift.registry.sources.location.url",
  "drift.notification.threadError",
  "drift.settings.section",
  "drift.settings.prompts.family.claude",
  "drift.settings.prompts.family.gemini",
])

type Catalog = { dict: Record<string, string>; drift: Record<string, string> }
const featureTranslations = (catalog: Catalog) =>
  Object.fromEntries(
    Object.entries({ ...catalog.dict, ...catalog.drift }).filter(([key]) => key.startsWith("drift.")),
  ) as Record<string, string>

test("no locale keeps a key English does not have", async () => {
  const { languages } = await import("../src/state/language")
  const en: Catalog = await import("../src/i18n/en")
  const english = new Set([...Object.keys(en.dict), ...Object.keys(en.drift)])
  for (const language of languages.filter((language) => language.id !== "en")) {
    const catalog: Catalog = await import(`../src/i18n/${language.id}.ts`)
    const extra = [...Object.keys(catalog.dict), ...Object.keys(catalog.drift)].filter((key) => !english.has(key))
    expect(extra, `${language.id} has keys en.ts dropped`).toEqual([])
  }
})

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

test("General settings expose preview modes and custom-only per-type toggles", async () => {
  const source = await Bun.file("src/ui/settings.tsx").text()
  const general = source.slice(
    source.indexOf("function GeneralSection()"),
    source.indexOf("function RemoteAccessSection()"),
  )
  expect(general).toContain('title={t("drift.preview.settings.title")}')
  expect(general).toContain('(["all", "none", "custom"] as const)')
  expect(general).toContain("selected={filePreviewPrefs().mode}")
  expect(general).toContain('<Show when={filePreviewPrefs().mode === "custom"}>')
  expect(general).toContain("<For each={filePreviewTypes}>")
  expect(general).toContain("checked={filePreviewPrefs().types[type]}")
  expect(general).toContain("setFilePreviewType(type, !filePreviewPrefs().types[type])")
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

test("notification defaults stay explicit and old webview auto-accept is forgotten only once the engine takes it", async () => {
  const { handOverAutoAccept, notificationDefaults, soundDefaults } = await import("../src/state/prefs")
  expect(notificationDefaults(true)).toEqual({ agent: true, permission: true, error: true })
  expect(soundDefaults()).toEqual({ agent: "none", permission: "none", error: "none" })
  const kept = new Map([
    ["drift.autoAccept.global", "true"],
    ["drift.autoAccept", JSON.stringify(["s1", 7, "s2"])],
  ])
  const storage = localStorage as Storage
  const saved = { getItem: storage.getItem, setItem: storage.setItem, removeItem: storage.removeItem }
  Object.assign(storage, {
    getItem: (key: string) => kept.get(key) ?? null,
    setItem: (key: string, value: string) => kept.set(key, value),
    removeItem: (key: string) => kept.delete(key),
  })
  try {
    await expect(handOverAutoAccept(async () => Promise.reject(new Error("engine down")))).rejects.toThrow(
      "engine down",
    )
    expect(kept.size, "a failed hand-over forgets nothing").toBe(2)
    const seen: unknown[] = []
    await handOverAutoAccept(async (offered) => (seen.push(offered), { all: false, sessions: ["s2"] }))
    expect(seen).toEqual([{ all: true, sessions: ["s1", "s2"] }])
    expect([...kept.entries()], "only what the engine did not take is offered again").toEqual([
      ["drift.autoAccept", JSON.stringify(["s2"])],
    ])
    await handOverAutoAccept(async () => ({ all: false, sessions: [] }))
    expect(kept.size).toBe(0)
  } finally {
    Object.assign(storage, saved)
  }
})

test("shell timeout preferences normalize and persist explicit no-timeout", async () => {
  const { normalizeShellTimeout, setShellTimeoutMs, shellTimeoutMs, shellTimeoutPresets } =
    await import("../src/state/prefs")
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
  mountScene(
    () => slow,
    () => {},
  )()
  land(() => disposed++)
  await settle()
  expect(disposed).toBe(1)

  let live = 0
  const cleanup = mountScene(
    () => Promise.resolve(() => live++),
    () => {},
  )
  await settle()
  expect(live).toBe(0)
  cleanup()
  cleanup()
  expect(live).toBe(1)

  let fallbacks = 0
  mountScene(
    () => Promise.reject(new Error("no webgl")),
    () => fallbacks++,
  )
  mountScene(
    () => Promise.resolve(undefined),
    () => fallbacks++,
  )
  await settle()
  expect(fallbacks).toBe(2)

  let ignored = 0
  mountScene(
    () => Promise.reject(new Error("no webgl")),
    () => ignored++,
  )()
  await settle()
  expect(ignored).toBe(0)
})

test("the mascot takes the theme accent and the logo mark never flashes as a block", async () => {
  const { applyAccent } = await import("../src/ui/jelly/jellyfish")
  const THREE = await import("three")

  // The bell palette derives from the accent, so a themed mascot never falls back to stock aqua.
  applyAccent("#c9a9e0")
  const bell = (await import("../src/ui/jelly/jellyfish")).createJellyfish()
  const tinted: string[] = []
  bell.group.traverse((object) => {
    const material = (object as { material?: unknown }).material as
      { uniforms?: Record<string, { value: unknown }> } | undefined
    for (const name of ["uTop", "uBottom", "uRim", "uColor", "uTip"]) {
      const value = material?.uniforms?.[name]?.value
      if (value instanceof THREE.Color) tinted.push(value.getHexString())
    }
  })
  expect(tinted.length).toBeGreaterThan(0)
  // Stock aqua (#8fd9fb and friends) must be gone entirely.
  expect(tinted).not.toContain("8fd9fb")
  expect(tinted).not.toContain("d4f2ff")
  expect(tinted).not.toContain("4f93cc")
  // Every bell tint stays on the accent hue rather than reverting to blue. The face colours are
  // deliberately fixed - the blush and eyes read as features, not as themed surfaces.
  const face = new Set(["ffa9b8", "ffffff", "0f1626"])
  const bellTints = tinted.filter((hex) => !face.has(hex))
  expect(bellTints.length).toBeGreaterThan(0)
  for (const hex of bellTints) {
    const color = new THREE.Color(`#${hex}`)
    const hsl = { h: 0, s: 0, l: 0 }
    color.getHSL(hsl)
    if (hsl.s > 0.05) expect(Math.abs(hsl.h - 0.763)).toBeLessThan(0.05)
  }

  // The logo mask is inlined, so `background: currentColor` is never painted unmasked.
  const logo = await Bun.file("src/ui/logo.tsx").text()
  expect(logo).toContain("logo.svg?raw")
  expect(logo).toContain("data:image/svg+xml,${encodeURIComponent(logoSource)}")
  const jelly = await Bun.file("src/ui/jellyfish.tsx").text()
  // Tinted before the first frame and retinted on theme changes, with the observer torn down.
  expect(jelly).toMatch(/applyAccent\(accentColor\(host\)\)[\s\S]*?jelly = createJellyfish\(\)/)
  expect(jelly).toContain("themeObserver.observe(document.documentElement")
  expect(jelly).toContain("themeObserver.disconnect()")
  expect(jelly).toContain("renderer.setClearColor(0x000000, 0)")

  // The canvas mounts hidden and is revealed only from inside render(), after a frame it drew.
  // Revealing at append time let WebView2 composite one opaque white frame first.
  expect(jelly).toContain('canvas.style.opacity = "0"')
  expect(jelly).toMatch(
    /renderer\.render\(scene, camera\)\s*\n[\s\S]*?if \(!revealed\) \{\s*\n\s*revealed = true\s*\n\s*canvas\.style\.opacity = "1"\s*\n\s*ready\(\)/,
  )
  // No reveal may happen next to the append, before any frame exists.
  expect(jelly).not.toMatch(/host\.append\(canvas\)\s*\n\s*ready\(\)/)
})

test("provider sign-in hides raw URLs, surfaces device codes, and keeps disconnect beside the methods", async () => {
  const { authorizationPrompt } = await import("../src/engine/provider-auth")
  const long = "https://claude.ai/oauth/authorize?code=true&client_id=9d1c&state=" + "x".repeat(400)
  expect(authorizationPrompt(`Paste the authorization code here: ${long}`)).toEqual({
    text: "Paste the authorization code here",
  })
  expect(authorizationPrompt("Enter code: ABCD-1234")).toEqual({ code: "ABCD-1234" })
  expect(authorizationPrompt("Open https://accounts.x.ai/device on any device and enter code: WXYZ-9876")).toEqual({
    code: "WXYZ-9876",
  })
  expect(authorizationPrompt("Sign in with `az login` before continuing.")).toEqual({
    text: "Sign in with `az login` before continuing.",
  })
  expect(authorizationPrompt("")).toEqual({})
  const source = await Bun.file("src/ui/settings.tsx").text()
  expect(source).not.toContain("{auth().url}")
  expect(source).not.toContain("disconnectDescription")
  const connect = source.slice(
    source.indexOf("function ProviderConnect("),
    source.indexOf("function AuthorizationHint("),
  )
  expect(connect.indexOf("props.methods.length > 1 || props.connected")).toBeLessThan(
    connect.indexOf('t("common.disconnect")'),
  )
  expect(connect.indexOf('t("common.disconnect")')).toBeLessThan(connect.indexOf('method()?.type === "api"'))
})

test("the About mascot stays light: preloaded from the nav, compiled off-thread, paced, and low-poly", async () => {
  const jelly = await Bun.file("src/ui/jellyfish.tsx").text()
  expect(jelly).toContain("await renderer.compileAsync(scene, camera)")
  expect(jelly).toContain('powerPreference: "low-power"')
  expect(jelly).toMatch(/if \(last && now - last < interval - 1\) return/)
  expect(jelly).toContain("renderer.setPixelRatio(1)")
  const settings = await Bun.file("src/ui/settings.tsx").text()
  expect(settings).toContain('onPointerEnter={() => name === "About" && void preloadJellyfish()')
  const { createJellyfish } = await import("../src/ui/jelly/jellyfish")
  const seen = new Set<unknown>()
  let vertices = 0
  createJellyfish().group.traverse((object) => {
    const geometry = (object as { geometry?: { attributes: { position: { count: number } } } }).geometry
    if (!geometry || seen.has(geometry)) return
    seen.add(geometry)
    vertices += geometry.attributes.position.count
  })
  expect(vertices).toBeGreaterThan(5_000)
  expect(vertices).toBeLessThan(12_000)
})

test("permission rules reorder within the list and grants read as what was approved", async () => {
  const { loadDictionary } = await import("../src/state/i18n")
  await loadDictionary("en")
  const { moveRule, grantLabel } = await import("../src/ui/settings-permissions")
  const rule = (pattern: string) => ({ kind: "bash", pattern, decision: "ask" as const })
  const rules = [rule("a"), rule("b"), rule("c")]
  expect(moveRule(rules, 2, -1).map((r) => r.pattern)).toEqual(["a", "c", "b"])
  expect(moveRule(rules, 0, -1).map((r) => r.pattern)).toEqual(["a", "b", "c"], "the first stays first")
  expect(moveRule(rules, 2, 1).map((r) => r.pattern)).toEqual(["a", "b", "c"])
  expect(grantLabel({ grant: "exact", kind: "edit", target: "src/[id].tsx" })).toBe("edit: src/[id].tsx")
  expect(grantLabel({ grant: "subcommand", prefix: "cargo test" })).toBe("bash: cargo test with any arguments")
  expect(grantLabel({ grant: "pattern", kind: "read", pattern: "docs/**", decision: "allow" })).toBe("read: docs/**")
})
test("always-allowed grants are grouped by what they let through and shown without the kind their group names", async () => {
  const { grantGroup, grantText } = await import("../src/ui/settings-permissions")
  const exact = { grant: "exact" as const, kind: "bash", target: "cargo test" }
  const sub = { grant: "subcommand" as const, prefix: "git push" }
  const folder = { grant: "folder" as const, kind: "read", folder: "C:/notes" }
  const site = { grant: "pattern" as const, kind: "webfetch", pattern: "https://docs.rs/*", decision: "allow" as const }
  expect([exact, sub, folder, site].map(grantGroup)).toEqual(["shell", "shell", "files", "web"])
  expect(grantText(exact)).toBe("cargo test")
  expect(grantText(site)).toBe("https://docs.rs/*")
})
