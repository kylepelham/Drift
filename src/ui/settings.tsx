import type { ProviderAuthMethod } from "@opencode-ai/sdk/client"
import { createEffect, createMemo, createSignal, For, Match, onCleanup, onMount, Show, Switch, type JSX } from "solid-js"
import { Portal } from "solid-js/web"
import { useEngine } from "../engine"
import {
  codeFontSize,
  codeFontSizes,
  codeTabWidth,
  codeTabWidths,
  codeWordWrap,
  diffIndicator,
  diffIndicators,
  diffLineNumbers,
  diffWordWrap,
  setCodeFontSize,
  setCodeTabWidth,
  setCodeWordWrap,
  setDiffIndicator,
  setDiffLineNumbers,
  setDiffWordWrap,
  setSyntaxThemePreset,
  syntaxThemePreset,
  syntaxThemePresets,
  type DiffIndicator,
  type SyntaxThemePreset,
} from "../state/code"
import { t } from "../state/i18n"
import { comboFor, eventCombo, formatCombo, keybindDefs, setCombo, type KeybindAction } from "../state/keybinds"
import { language, languages, setLanguage, type LanguageId } from "../state/language"
import {
  alertSounds,
  attentionKinds,
  autoAcceptGlobal,
  autoUpdate,
  collapseCompaction,
  compactionCollapsed,
  customSound,
  setAlertSound,
  setAutoAcceptGlobal,
  setAutoUpdate,
  setCollapseCompaction,
  setCompactionCollapsed,
  setCustomSound,
  setShowReasoning,
  setSystemNotification,
  setToolErrorsExpanded,
  showReasoning,
  systemNotifications,
  toolErrorsExpanded,
  type AttentionKind,
} from "../state/prefs"
import { shellInvoke } from "../state/store"
import { requestNotificationPermission } from "./notifications"
import {
  codeFont,
  customCss,
  customTheme,
  setCodeFont,
  setCustomCss,
  setCustomThemeColor,
  setTheme,
  setUiFont,
  theme,
  themes,
  uiFont,
  type CustomTheme,
  type ThemeName,
} from "../state/theme"
import {
  IconBell,
  IconCheck,
  IconChip,
  IconCode,
  IconInfo,
  IconKeyboard,
  IconPalette,
  IconPlus,
  IconShieldCheck,
  IconSliders,
  IconX,
} from "./icons"
import { activateModal, closeOnBackdropPointerDown } from "./modal"
import { McpManagement } from "./mcp"
import { Toggle } from "./model-manager"
import { ProviderIcon } from "./provider-icon"
import { Picker } from "./picker"
import { playAlertSound, soundOptions } from "./sounds"

type ProviderNotice = { tone: "success" | "warning" | "error"; text: string }

const themeMeta: Record<ThemeName, { label: string; swatch: [string, string, string] }> = {
  "drift-dark": { label: "drift.theme.dark", swatch: ["#141517", "#212429", "#7ba3e8"] },
  "drift-graphite": { label: "drift.theme.graphite", swatch: ["#101112", "#222326", "#b7b9c2"] },
  "drift-midnight": { label: "drift.theme.midnight", swatch: ["#0c1020", "#19223a", "#8aa8ff"] },
  "drift-slate": { label: "drift.theme.slate", swatch: ["#0f1419", "#1b232c", "#6cb2c9"] },
  "drift-forest": { label: "drift.theme.forest", swatch: ["#0f1512", "#1d2922", "#82c99a"] },
  "drift-aubergine": { label: "drift.theme.aubergine", swatch: ["#171119", "#2d2031", "#d29ad8"] },
  "drift-light": { label: "drift.theme.light", swatch: ["#f4f4f5", "#ffffff", "#3a6fd8"] },
  "drift-paper": { label: "drift.theme.paper", swatch: ["#eee9df", "#fffdf8", "#97643c"] },
  "drift-custom": { label: "drift.theme.custom", swatch: ["#111318", "#1b1e25", "#a78bfa"] },
}

const sections = ["General", "Appearance", "Code", "Notifications", "Shortcuts", "Providers", "MCP", "About"] as const
type Section = (typeof sections)[number]
const sectionLabels: Record<Section, string> = {
  General: "settings.tab.general",
  Appearance: "settings.general.section.appearance",
  Code: "drift.settings.code",
  Notifications: "drift.settings.notifications",
  Shortcuts: "settings.tab.shortcuts",
  Providers: "settings.providers.title",
  MCP: "dialog.mcp.title",
  About: "drift.settings.about",
}
const sectionGroups: { label: string; items: Section[] }[] = [
  { label: "settings.section.desktop", items: ["General", "Appearance", "Code", "Notifications", "Shortcuts"] },
  { label: "settings.section.server", items: ["Providers", "MCP"] },
  { label: "drift.settings.section", items: ["About"] },
]

const keybindLabels: Record<KeybindAction, string> = {
  palette: "command.palette",
  newThread: "command.session.new",
  autoAccept: "drift.shortcuts.autoAccept",
  zoomIn: "drift.shortcuts.zoomIn",
  zoomOut: "drift.shortcuts.zoomOut",
  zoomReset: "drift.shortcuts.zoomReset",
}

const [settingsOpen, setSettingsOpen] = createSignal(false)
const [settingsSection, setSettingsSection] = createSignal<Section>("General")

export function openSettings(section?: Section) {
  setSettingsSection(section && sections.includes(section) ? section : "General")
  setSettingsOpen(true)
}

export function SettingsHost() {
  return (
    <Show when={settingsOpen()}>
      <Portal>
        <SettingsModal onClose={() => setSettingsOpen(false)} />
      </Portal>
    </Show>
  )
}

function SettingsModal(props: { onClose: () => void }) {
  let dialog!: HTMLDivElement
  const section = settingsSection
  onMount(() => onCleanup(activateModal(dialog, props.onClose)))

  return (
    <div
      data-modal-layer
      class="fixed inset-0 z-30 flex items-center justify-center bg-black/50"
      onPointerDown={(event) => closeOnBackdropPointerDown(event, props.onClose, dialog)}
    >
      <div
        ref={dialog}
        role="dialog"
        aria-modal="true"
        aria-label={t("sidebar.settings")}
        tabIndex={-1}
        class="fade-up flex h-[calc(100vh-1rem)] w-[calc(100vw-1rem)] overflow-hidden rounded-xl border border-edge bg-overlay shadow-2xl shadow-black/40 sm:h-[min(42rem,calc(100vh-3rem))] sm:w-[min(54rem,calc(100vw-3rem))]"
        onClick={(event) => event.stopPropagation()}
      >
        <nav class="flex w-13 shrink-0 flex-col overflow-hidden border-r border-edge px-1.5 py-3 sm:w-44 sm:px-3">
          <For each={sectionGroups}>
            {(group) => (
              <div class="mb-3 last:mb-0">
                <div class="hidden px-2 pb-1.5 text-[0.68rem] font-medium text-ink-faint sm:block">{t(group.label)}</div>
                <div class="space-y-0.5">
                  <For each={group.items}>
                    {(name) => (
                      <button
                        aria-label={t(sectionLabels[name])}
                        class="flex w-full items-center justify-center gap-2.5 rounded-md px-2 py-1.5 text-left text-sm transition-colors sm:justify-start"
                        classList={{
                          "bg-raised text-ink": section() === name,
                          "text-ink-muted hover:bg-raised/60 hover:text-ink": section() !== name,
                        }}
                        onClick={() => setSettingsSection(name)}
                      >
                        <SectionIcon section={name} />
                        <span class="hidden min-w-0 truncate sm:inline" title={t(sectionLabels[name])}>
                          {t(sectionLabels[name])}
                        </span>
                      </button>
                    )}
                  </For>
                </div>
              </div>
            )}
          </For>
        </nav>
        <div class="flex min-w-0 flex-1 flex-col">
          <div class="settings-header z-10 flex items-center justify-between px-5 py-3.5">
            <span class="min-w-0 truncate text-sm font-semibold text-ink">{t(sectionLabels[section()])}</span>
            <button
              title={t("common.close")}
              class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
              onClick={props.onClose}
            >
              <IconX />
            </button>
          </div>
          <div class="min-h-0 flex-1 overflow-y-auto px-3 py-4 sm:px-4">
            <Switch>
              <Match when={section() === "General"}>
                <GeneralSection />
              </Match>
              <Match when={section() === "Appearance"}>
                <AppearanceSection />
              </Match>
              <Match when={section() === "Code"}>
                <CodeSection />
              </Match>
              <Match when={section() === "Notifications"}>
                <NotificationsSection />
              </Match>
              <Match when={section() === "Providers"}>
                <ProvidersSection />
              </Match>
              <Match when={section() === "MCP"}>
                <McpManagement embedded />
              </Match>
              <Match when={section() === "Shortcuts"}>
                <KeybindsSection />
              </Match>
              <Match when={section() === "About"}>
                <AboutSection />
              </Match>
            </Switch>
          </div>
        </div>
      </div>
    </div>
  )
}

function GeneralSection() {
  return (
    <div class="space-y-5">
      <SettingsGroup title={t("settings.general.section.display")}>
        <SettingsRow
          title={t("settings.general.row.language.title")}
          description={t("settings.general.row.language.description")}
        >
          <Picker
            label={t("settings.general.row.language.title")}
            items={languages.map((item) => ({ id: item.id, label: item.label }))}
            selected={language()}
            floating
            bordered
            chevronAtEnd
            placement="below"
            width="12rem"
            onPick={(value) => setLanguage(value as LanguageId)}
          />
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup title={t("settings.agents.title")}>
        <SettingsRow
          title={t("command.permissions.autoaccept.enable")}
          description={t("toast.permissions.autoaccept.on.description")}
          onClick={() => setAutoAcceptGlobal(!autoAcceptGlobal())}
        >
          <Toggle
            label={t("command.permissions.autoaccept.enable")}
            checked={autoAcceptGlobal()}
            onChange={() => setAutoAcceptGlobal(!autoAcceptGlobal())}
          />
        </SettingsRow>
        <SettingsRow
          title={t("settings.general.row.reasoningSummaries.title")}
          description={t("settings.general.row.reasoningSummaries.description")}
          onClick={() => setShowReasoning(!showReasoning())}
        >
          <Toggle
            label={t("settings.general.row.reasoningSummaries.title")}
            checked={showReasoning()}
            onChange={() => setShowReasoning(!showReasoning())}
          />
        </SettingsRow>
        <SettingsRow
          title={t("drift.settings.toolErrors.title")}
          description={t("drift.settings.toolErrors.description")}
          onClick={() => setToolErrorsExpanded(!toolErrorsExpanded())}
        >
          <Toggle
            label={t("drift.settings.toolErrors.title")}
            checked={toolErrorsExpanded()}
            onChange={() => setToolErrorsExpanded(!toolErrorsExpanded())}
          />
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup title={t("drift.settings.summaries")}>
        <SettingsRow
          title={t("drift.settings.summaries.collapsible.title")}
          description={t("drift.settings.summaries.collapsible.description")}
          onClick={() => setCollapseCompaction(!collapseCompaction())}
        >
          <Toggle
            label={t("drift.settings.summaries.collapsible.title")}
            checked={collapseCompaction()}
            onChange={() => setCollapseCompaction(!collapseCompaction())}
          />
        </SettingsRow>
        <SettingsRow
          title={t("drift.settings.summaries.collapsed.title")}
          description={t("drift.settings.summaries.collapsed.description")}
          disabled={!collapseCompaction()}
          onClick={() => collapseCompaction() && setCompactionCollapsed(!compactionCollapsed())}
        >
          <Toggle
            label={t("drift.settings.summaries.collapsed.title")}
            checked={compactionCollapsed()}
            disabled={!collapseCompaction()}
            onChange={() => setCompactionCollapsed(!compactionCollapsed())}
          />
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup title={t("settings.general.section.updates")}>
        <SettingsRow
          title={t("settings.updates.row.startup.title")}
          description={t("settings.updates.row.startup.description")}
          onClick={() => setAutoUpdate(!autoUpdate())}
        >
          <Toggle
            label={t("settings.updates.row.startup.title")}
            checked={autoUpdate()}
            onChange={() => setAutoUpdate(!autoUpdate())}
          />
        </SettingsRow>
      </SettingsGroup>
    </div>
  )
}

const notificationKeys: Record<AttentionKind, string> = {
  agent: "agent",
  permission: "permissions",
  error: "errors",
}

function NotificationsSection() {
  return (
    <div class="space-y-5">
      <SettingsGroup title={t("settings.general.section.notifications")}>
        <For each={attentionKinds}>
          {(kind) => {
            const toggle = () => {
              const next = !systemNotifications()[kind]
              if (next) requestNotificationPermission()
              setSystemNotification(kind, next)
            }
            return (
              <SettingsRow
                title={t(`settings.general.notifications.${notificationKeys[kind]}.title`)}
                description={t(`settings.general.notifications.${notificationKeys[kind]}.description`)}
                onClick={toggle}
              >
                <Toggle
                  label={t(`settings.general.notifications.${notificationKeys[kind]}.title`)}
                  checked={!!systemNotifications()[kind]}
                  onChange={toggle}
                />
              </SettingsRow>
            )
          }}
        </For>
      </SettingsGroup>

      <SettingsGroup title={t("settings.general.section.sounds")}>
        <For each={attentionKinds}>
          {(kind) => (
            <SettingsRow
              title={t(`settings.general.sounds.${notificationKeys[kind]}.title`)}
              description={t(`settings.general.sounds.${notificationKeys[kind]}.description`)}
            >
              <SoundPicker kind={kind} />
            </SettingsRow>
          )}
        </For>
      </SettingsGroup>
    </div>
  )
}

function SoundPicker(props: { kind: AttentionKind }) {
  let picker!: HTMLInputElement
  const [error, setError] = createSignal("")
  const options = createMemo(() => [
    { id: "none", label: t("sound.option.none") },
    ...soundOptions.map((item) => ({
      id: item.id,
      label: item.label,
      group: item.group,
    })),
    ...(customSound() ? [{ id: "custom", label: `${t("prompt.slash.badge.custom")}: ${customSound()!.name}` }] : []),
  ])

  async function upload(file: File | undefined) {
    if (!file) return
    if (!file.type.startsWith("audio/")) return setError(t("drift.settings.sound.audioFileRequired"))
    if (file.size > 1024 * 1024) return setError(t("drift.settings.sound.maxSize"))
    const dataUrl = await readDataUrl(file)
    const sound = { name: file.name, dataUrl }
    setCustomSound(sound)
    setAlertSound(props.kind, "custom")
    setError("")
    void playAlertSound("custom", sound)
  }

  return (
    <div class="flex min-w-0 items-center gap-1.5" title={error() || undefined}>
      <Picker
        label={`${t(`settings.general.sounds.${notificationKeys[props.kind]}.title`)} ${t("settings.general.section.sounds")}`}
        items={options()}
        selected={alertSounds()[props.kind] ?? "none"}
        floating
        bordered
        chevronAtEnd
        placement="below"
        width="9.5rem"
        onPick={(id) => {
          setAlertSound(props.kind, id)
          void playAlertSound(id, customSound())
        }}
      />
      <input
        ref={picker}
        type="file"
        accept="audio/*,.aac,.mp3,.wav,.ogg,.m4a"
        class="hidden"
        onChange={(event) => {
          void upload(event.currentTarget.files?.[0])
          event.currentTarget.value = ""
        }}
      />
      <button
        title={t("drift.settings.sound.chooseCustom")}
        class="flex size-8 shrink-0 items-center justify-center rounded-md border border-edge text-ink-muted transition-colors hover:border-edge-strong hover:text-ink"
        onClick={() => picker.click()}
      >
        <IconPlus class="size-3.5" />
      </button>
    </div>
  )
}

function readDataUrl(file: File) {
  return new Promise<string>((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(reader.result as string)
    reader.onerror = () => reject(reader.error)
    reader.readAsDataURL(file)
  })
}

function ProvidersSection() {
  const engine = useEngine()
  const [methods, setMethods] = createSignal<Record<string, ProviderAuthMethod[]>>({})
  const [expanded, setExpanded] = createSignal<string | null>(null)
  const [query, setQuery] = createSignal("")
  const [notice, setNotice] = createSignal<ProviderNotice | null>(null)
  onMount(() => void engine.actions.providerAuthMethods().then((map) => setMethods({ ...map })))

  const groups = createMemo(() => {
    const value = query().toLowerCase()
    const matching = engine.state.providers
      .filter((provider) => provider.name.toLowerCase().includes(value) || provider.id.toLowerCase().includes(value))
      .sort((a, b) => a.name.localeCompare(b.name))
    return {
      connected: matching.filter((provider) => engine.state.connected.includes(provider.id)),
      rest: matching.filter((provider) => !engine.state.connected.includes(provider.id)),
    }
  })

  const row = (provider: (typeof engine.state.providers)[number]) => (
    <div class="rounded-lg" classList={{ "bg-raised/40": expanded() === provider.id }}>
      <div
        class="flex cursor-pointer items-center gap-2.5 rounded-lg px-3 py-2 hover:bg-raised/60"
        onClick={() => setExpanded(expanded() === provider.id ? null : provider.id)}
      >
        <ProviderIcon id={provider.id} class="size-4 shrink-0" />
        <span class="min-w-0 flex-1 truncate text-sm text-ink">{provider.name}</span>
        <span
          class="size-1.5 shrink-0 rounded-full"
          classList={{
            "bg-ok": engine.state.connected.includes(provider.id),
            "bg-ink-faint": !engine.state.connected.includes(provider.id),
          }}
        />
      </div>
      <Show when={expanded() === provider.id}>
        <ProviderConnect
          providerId={provider.id}
          providerName={provider.name}
          connected={engine.state.connected.includes(provider.id)}
          methods={methods()[provider.id] ?? [{ type: "api", label: t("provider.connect.method.apiKey") }]}
          onNotice={setNotice}
        />
      </Show>
    </div>
  )

  return (
    <div class="space-y-1">
      <input
        class="mb-2 w-full rounded-lg border border-edge bg-surface px-3 py-2 text-sm outline-none placeholder:text-ink-faint focus:border-edge-strong"
        placeholder={t("dialog.provider.search.placeholder")}
        value={query()}
        onInput={(event) => setQuery(event.currentTarget.value)}
      />
      <Show when={notice()}>
        {(item) => (
          <div
            class="mb-2 rounded-md border px-3 py-2 text-xs"
            classList={{
              "border-ok/35 bg-ok/10 text-ok": item().tone === "success",
              "border-warn/35 bg-warn/10 text-warn": item().tone === "warning",
              "border-danger/35 bg-danger/10 text-danger": item().tone === "error",
            }}
          >
            {item().text}
          </div>
        )}
      </Show>
      <Show when={groups().connected.length > 0}>
        <div class="px-3 pt-1 pb-1 text-[0.68rem] tracking-wider text-ink-faint uppercase">
          {t("settings.providers.section.connected")}
        </div>
        <For each={groups().connected}>{row}</For>
      </Show>
      <Show when={groups().rest.length > 0}>
        <div class="px-3 pt-3 pb-1 text-[0.68rem] tracking-wider text-ink-faint uppercase">
          {t("drift.settings.providers.notConnected")}
        </div>
        <For each={groups().rest}>{row}</For>
      </Show>
      <Show when={groups().connected.length === 0 && groups().rest.length === 0}>
        <div class="px-3 py-4 text-sm text-ink-faint">{t("dialog.provider.empty")}</div>
      </Show>
    </div>
  )
}

function ProviderConnect(props: {
  providerId: string
  providerName: string
  connected: boolean
  methods: ProviderAuthMethod[]
  onNotice: (notice: ProviderNotice) => void
}) {
  const engine = useEngine()
  const [methodIndex, setMethodIndex] = createSignal(0)
  const [key, setKey] = createSignal("")
  const [code, setCode] = createSignal("")
  const [pending, setPending] = createSignal<"connect" | "disconnect" | null>(null)
  const [error, setError] = createSignal("")
  const [authorization, setAuthorization] = createSignal<{ url: string; method: string; instructions: string } | null>(null)
  const method = () => props.methods[methodIndex()] ?? props.methods[0]

  function fail(message: string) {
    setError(message)
    setPending(null)
    props.onNotice({ tone: "error", text: message })
  }

  async function finish(result: { ok: boolean; connected: boolean }) {
    if (!result.ok) {
      fail(t("drift.provider.connectFailed", { provider: props.providerName }))
      return
    }
    if (!result.connected) {
      fail(t("drift.provider.savedUnavailable", { provider: props.providerName }))
      return
    }
    setKey("")
    setCode("")
    setPending(null)
    props.onNotice({ tone: "success", text: t("drift.provider.connected", { provider: props.providerName }) })
  }

  async function connectApi() {
    if (!key().trim()) return
    setPending("connect")
    setError("")
    await finish(await engine.actions.setProviderKey(props.providerId, key().trim()))
  }

  async function startOauth() {
    setPending("connect")
    setError("")
    const auth = await engine.actions.providerAuthorize(props.providerId, methodIndex()).catch(() => null)
    if (!auth) {
      setError(t("drift.provider.signInStartFailed"))
      setPending(null)
      props.onNotice({ tone: "error", text: t("drift.provider.signInStartFailedFor", { provider: props.providerName }) })
      return
    }
    setAuthorization(auth)
    openExternal(auth.url)
    if (auth.method === "auto") {
      await finish(await engine.actions.providerCallback(props.providerId, methodIndex()))
      setAuthorization(null)
      return
    }
    setPending(null)
  }

  async function submitCode() {
    if (!code().trim()) return
    setPending("connect")
    setError("")
    await finish(await engine.actions.providerCallback(props.providerId, methodIndex(), code().trim()))
  }

  async function disconnect() {
    setPending("disconnect")
    setError("")
    const result = await engine.actions.disconnectProvider(props.providerId)
    setPending(null)
    if (!result.ok) {
      fail(t("drift.provider.disconnectFailed", { provider: props.providerName }))
      return
    }
    if (result.connected) {
      props.onNotice({
        tone: "warning",
        text: t("drift.provider.credentialRemovedStillConnected", { provider: props.providerName }),
      })
      return
    }
    props.onNotice({ tone: "success", text: t("drift.provider.disconnected", { provider: props.providerName }) })
  }

  return (
    <div class="space-y-2 px-3 pt-1 pb-3">
      <Show when={props.methods.length > 1}>
        <div class="flex gap-1.5">
          <For each={props.methods}>
            {(item, index) => (
              <button
                class="rounded-md border px-2 py-0.5 text-xs transition-colors"
                classList={{
                  "border-accent text-ink": index() === methodIndex(),
                  "border-edge text-ink-muted hover:text-ink": index() !== methodIndex(),
                }}
                onClick={() => {
                  setMethodIndex(index())
                  setAuthorization(null)
                  setError("")
                }}
              >
                {item.label}
              </button>
            )}
          </For>
        </div>
      </Show>
      <Show when={method()?.type === "api"}>
        <div class="flex gap-2">
          <input
            type="password"
            class="min-w-0 flex-1 rounded-md border border-edge bg-surface px-2.5 py-1.5 text-sm outline-none focus:border-edge-strong"
            placeholder={t("provider.connect.apiKey.placeholder")}
            value={key()}
            onInput={(event) => setKey(event.currentTarget.value)}
            onKeyDown={(event) => event.key === "Enter" && void connectApi()}
          />
          <button
            class="rounded-md bg-accent px-3 py-1 text-xs font-medium text-accent-ink disabled:opacity-40"
            disabled={pending() !== null || !key().trim()}
            onClick={() => void connectApi()}
          >
              {pending() === "connect" ? t("provider.connect.status.inProgress") : props.connected ? t("common.save") : t("common.connect")}
          </button>
        </div>
      </Show>
      <Show when={method()?.type === "oauth"}>
        <Show
          when={authorization()}
          fallback={
            <button
              class="rounded-md bg-accent px-3 py-1 text-xs font-medium text-accent-ink disabled:opacity-40"
              disabled={pending() !== null}
              onClick={() => void startOauth()}
            >
              {pending() === "connect"
                ? t("provider.connect.status.waiting")
                : t("drift.provider.signInWith", { method: method()?.label ?? t("drift.provider.browser") })}
            </button>
          }
        >
          {(auth) => (
            <div class="space-y-2">
              <div class="text-xs text-ink-muted">{auth().instructions || t("drift.provider.finishInBrowser")}</div>
              <div class="text-xs break-all text-ink-faint select-text">{auth().url}</div>
              <Show when={auth().method === "code"}>
                <div class="flex gap-2">
                  <input
                    class="min-w-0 flex-1 rounded-md border border-edge bg-surface px-2.5 py-1.5 text-sm outline-none focus:border-edge-strong"
                    placeholder={t("provider.connect.oauth.code.placeholder")}
                    value={code()}
                    onInput={(event) => setCode(event.currentTarget.value)}
                    onKeyDown={(event) => event.key === "Enter" && void submitCode()}
                  />
                  <button
                    class="rounded-md bg-accent px-3 py-1 text-xs font-medium text-accent-ink disabled:opacity-40"
                    disabled={pending() !== null || !code().trim()}
                    onClick={() => void submitCode()}
                  >
                    {pending() === "connect" ? t("provider.connect.status.inProgress") : t("common.submit")}
                  </button>
                </div>
              </Show>
              <Show when={auth().method === "auto" && pending() === "connect"}>
                <div class="pulse-soft text-xs text-ink-faint">{t("provider.connect.status.waiting")}</div>
              </Show>
            </div>
          )}
        </Show>
      </Show>
      <Show when={error()}>
        <div class="text-xs text-danger">{error()}</div>
      </Show>
      <Show when={props.connected}>
        <div class="flex items-center justify-between gap-3 border-t border-edge pt-2">
          <span class="text-xs text-ink-faint">{t("drift.provider.disconnectDescription")}</span>
          <button
            class="rounded-md border border-danger/40 px-2.5 py-1 text-xs text-danger transition-colors hover:bg-danger/10 disabled:opacity-40"
            disabled={pending() !== null}
            onClick={() => void disconnect()}
          >
            {pending() === "disconnect" ? t("drift.provider.disconnecting") : t("common.disconnect")}
          </button>
        </div>
      </Show>
    </div>
  )
}

function openExternal(url: string) {
  const invoke = shellInvoke()
  if (invoke) {
    void invoke("plugin:opener|open_url", { url }).catch(() => {})
    return
  }
  window.open(url, "_blank")
}

function KeybindsSection() {
  const [capturing, setCapturing] = createSignal<KeybindAction | null>(null)

  createEffect(() => {
    const action = capturing()
    if (!action) return
    const capture = (event: KeyboardEvent) => {
      event.preventDefault()
      event.stopPropagation()
      if (event.key === "Escape") return setCapturing(null)
      const combo = eventCombo(event)
      if (!combo) return
      setCombo(action, combo)
      setCapturing(null)
    }
    document.addEventListener("keydown", capture, true)
    onCleanup(() => document.removeEventListener("keydown", capture, true))
  })

  return (
    <div class="space-y-1">
      <For each={keybindDefs}>
        {(def) => (
          <div class="flex items-center gap-2 rounded-lg px-3 py-2 hover:bg-raised/60">
            <span class="min-w-0 flex-1 truncate text-sm text-ink">{t(keybindLabels[def.action])}</span>
            <button
              class="rounded-md border px-2.5 py-1 font-mono text-xs transition-colors"
              classList={{
                "border-accent text-accent": capturing() === def.action,
                "border-edge text-ink-muted hover:border-edge-strong hover:text-ink": capturing() !== def.action,
              }}
              onClick={() => setCapturing(capturing() === def.action ? null : def.action)}
            >
              {capturing() === def.action
                ? `${t("settings.shortcuts.pressKeys")}...`
                : comboFor(def.action)
                  ? formatCombo(comboFor(def.action))
                  : t("settings.shortcuts.unassigned")}
            </button>
            <button
              title={t("settings.shortcuts.unassigned")}
              class="flex size-6 shrink-0 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
              onClick={() => setCombo(def.action, null)}
            >
              <IconX class="size-3.5" />
            </button>
          </div>
        )}
      </For>
    </div>
  )
}

function AboutSection() {
  const engine = useEngine()
  return (
    <div class="space-y-1 px-3 text-sm text-ink-muted select-text">
      <div>{t("drift.about.description")}</div>
      <div class="text-xs text-ink-faint">
        {t("drift.about.engine")} opencode{" "}
        {engine.state.version || (engine.state.startupError ? t("drift.about.failed") : t("drift.about.starting"))}
      </div>
      <Show when={engine.state.startupError}>
        <div class="text-xs text-danger">{engine.state.startupError}</div>
      </Show>
    </div>
  )
}

const customColorMeta: { id: keyof CustomTheme; label: string }[] = [
  { id: "background", label: "drift.color.background" },
  { id: "surface", label: "drift.color.surface" },
  { id: "text", label: "drift.color.text" },
  { id: "accent", label: "drift.color.accent" },
]

function AppearanceSection() {
  return (
    <div class="space-y-6">
      <SettingsGroup title={t("settings.general.row.theme.title")}>
        <div class="space-y-0.5 py-1">
          <For each={themes}>{(name) => <ThemeRow name={name} />}</For>
        </div>
      </SettingsGroup>

      <Show when={theme() === "drift-custom"}>
      <SettingsGroup title={t("drift.settings.customPalette")}>
          <For each={customColorMeta}>
            {(color) => (
              <SettingsRow
                title={t(color.label)}
                description={t("drift.settings.customPalette.colorDescription", { color: t(color.label).toLowerCase() })}
              >
                <div class="flex items-center gap-2">
                  <input
                    type="color"
                    aria-label={t("dialog.project.edit.color.select", { color: t(color.label) })}
                    class="size-7 cursor-pointer rounded border border-edge bg-transparent p-0.5"
                    value={customTheme()[color.id]}
                    onInput={(event) => setCustomThemeColor(color.id, event.currentTarget.value)}
                  />
                  <input
                    aria-label={t("drift.settings.customPalette.hexValue", { color: t(color.label) })}
                    class="h-8 w-24 rounded-md border border-edge bg-raised/45 px-2 font-mono text-xs text-ink outline-none focus:border-accent"
                    value={customTheme()[color.id]}
                    onInput={(event) => setCustomThemeColor(color.id, event.currentTarget.value)}
                  />
                </div>
              </SettingsRow>
            )}
          </For>
        </SettingsGroup>
      </Show>

      <SettingsGroup title={t("drift.settings.typography")}>
        <SettingsRow
          title={t("settings.general.row.uiFont.title")}
          description={t("settings.general.row.uiFont.description")}
        >
          <FontField label={t("settings.general.row.uiFont.title")} value={uiFont()} onInput={setUiFont} />
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup title={t("drift.settings.customCss")}>
        <div class="py-2">
          <div class="mb-2 text-xs leading-relaxed text-ink-faint">{t("drift.settings.customCss.description")}</div>
          <textarea
            aria-label={t("drift.settings.customCss")}
            class="h-32 w-full resize-y rounded-lg border border-edge bg-bg/50 p-3 font-mono text-xs leading-relaxed text-ink outline-none placeholder:text-ink-faint focus:border-accent"
            placeholder=":root { --accent: #8aa8ff; }"
            spellcheck={false}
            value={customCss()}
            onInput={(event) => setCustomCss(event.currentTarget.value)}
          />
        </div>
      </SettingsGroup>
    </div>
  )
}

const syntaxThemeLabels: Record<SyntaxThemePreset, string> = {
  automatic: "drift.code.theme.automatic",
  github: "drift.code.theme.github",
  vitesse: "drift.code.theme.vitesse",
  one: "drift.code.theme.one",
  dracula: "drift.code.theme.dracula",
  nord: "drift.code.theme.nord",
}
const diffIndicatorLabels: Record<DiffIndicator, string> = {
  symbols: "drift.code.diffIndicators.symbols",
  bars: "drift.code.diffIndicators.bars",
  background: "drift.code.diffIndicators.background",
}

export function codeSettingOptions() {
  return {
    themes: [...syntaxThemePresets],
    fontSizes: [...codeFontSizes],
    tabWidths: [...codeTabWidths],
    indicators: [...diffIndicators],
  }
}

function CodeSection() {
  return (
    <div class="space-y-6">
      <SettingsGroup title={t("drift.code.syntax")}>
        <SettingsRow title={t("drift.code.syntaxTheme.title")} description={t("drift.code.syntaxTheme.description")}>
          <Picker
            label={t("drift.code.syntaxTheme.title")}
            items={syntaxThemePresets.map((name) => ({ id: name, label: t(syntaxThemeLabels[name]) }))}
            selected={syntaxThemePreset()}
            floating bordered chevronAtEnd placement="below" width="12rem"
            onPick={(value) => setSyntaxThemePreset(value as SyntaxThemePreset)}
          />
        </SettingsRow>
        <SettingsRow title={t("settings.general.row.font.title")} description={t("settings.general.row.font.description")}>
          <FontField label={t("settings.general.row.font.title")} value={codeFont()} onInput={setCodeFont} mono />
        </SettingsRow>
      </SettingsGroup>
      <SettingsGroup title={t("drift.code.layout")}>
        <SettingsRow title={t("drift.code.fontSize.title")} description={t("drift.code.fontSize.description")}>
          <Picker
            label={t("drift.code.fontSize.title")}
            items={codeFontSizes.map((size) => ({ id: String(size), label: `${size} px` }))}
            selected={String(codeFontSize())}
            floating bordered chevronAtEnd placement="below" width="8rem"
            onPick={(value) => setCodeFontSize(Number(value))}
          />
        </SettingsRow>
        <SettingsRow title={t("drift.code.tabWidth.title")} description={t("drift.code.tabWidth.description")}>
          <Picker
            label={t("drift.code.tabWidth.title")}
            items={codeTabWidths.map((width) => ({ id: String(width), label: t("drift.code.spaces", { count: width }) }))}
            selected={String(codeTabWidth())}
            floating bordered chevronAtEnd placement="below" width="9rem"
            onPick={(value) => setCodeTabWidth(Number(value))}
          />
        </SettingsRow>
        <SettingsRow title={t("drift.code.wordWrap.title")} description={t("drift.code.wordWrap.description")} onClick={() => setCodeWordWrap(!codeWordWrap())}>
          <Toggle label={t("drift.code.wordWrap.title")} checked={codeWordWrap()} onChange={() => setCodeWordWrap(!codeWordWrap())} />
        </SettingsRow>
      </SettingsGroup>
      <SettingsGroup title={t("drift.code.diffs")}>
        <SettingsRow title={t("drift.code.diffWordWrap.title")} description={t("drift.code.diffWordWrap.description")} onClick={() => setDiffWordWrap(!diffWordWrap())}>
          <Toggle label={t("drift.code.diffWordWrap.title")} checked={diffWordWrap()} onChange={() => setDiffWordWrap(!diffWordWrap())} />
        </SettingsRow>
        <SettingsRow title={t("drift.code.lineNumbers.title")} description={t("drift.code.lineNumbers.description")} onClick={() => setDiffLineNumbers(!diffLineNumbers())}>
          <Toggle label={t("drift.code.lineNumbers.title")} checked={diffLineNumbers()} onChange={() => setDiffLineNumbers(!diffLineNumbers())} />
        </SettingsRow>
        <SettingsRow title={t("drift.code.diffIndicator.title")} description={t("drift.code.diffIndicator.description")}>
          <Picker
            label={t("drift.code.diffIndicator.title")}
            items={diffIndicators.map((name) => ({ id: name, label: t(diffIndicatorLabels[name]) }))}
            selected={diffIndicator()}
            floating bordered chevronAtEnd placement="below" width="10rem"
            onPick={(value) => setDiffIndicator(value as DiffIndicator)}
          />
        </SettingsRow>
      </SettingsGroup>
    </div>
  )
}

function FontField(props: { label: string; value: string; onInput: (value: string) => void; mono?: boolean }) {
  return (
    <input
      aria-label={props.label}
      class="h-8 w-full rounded-md border border-edge bg-raised/45 px-2.5 text-sm text-ink outline-none placeholder:text-ink-faint focus:border-accent sm:w-56"
      classList={{ "font-mono text-xs": props.mono }}
      placeholder={props.mono ? '"Cascadia Code", monospace' : '"Segoe UI", sans-serif'}
      value={props.value}
      onInput={(event) => props.onInput(event.currentTarget.value)}
    />
  )
}

function SectionIcon(props: { section: Section }) {
  const icon = () => {
    if (props.section === "General") return <IconSliders />
    if (props.section === "Appearance") return <IconPalette />
    if (props.section === "Code") return <IconCode />
    if (props.section === "Notifications") return <IconBell />
    if (props.section === "Shortcuts") return <IconKeyboard />
    if (props.section === "Providers") return <IconChip />
    if (props.section === "MCP") return <IconShieldCheck />
    return <IconInfo />
  }
  return <span class="flex size-5 shrink-0 items-center justify-center text-ink-faint">{icon()}</span>
}

function SettingsGroup(props: { title: string; children: JSX.Element }) {
  return (
    <section>
      <div class="mb-1.5 text-[0.68rem] font-semibold tracking-wide text-ink-faint uppercase">{props.title}</div>
      <div class="border-y border-edge/80">{props.children}</div>
    </section>
  )
}

function SettingsRow(props: {
  title: string
  description: string
  children: JSX.Element
  disabled?: boolean
  onClick?: () => void
}) {
  return (
    <div
      class="flex min-h-13 flex-col items-stretch gap-2 border-b border-edge/70 px-1 py-2.5 last:border-b-0 sm:flex-row sm:items-center sm:gap-4"
      classList={{
        "cursor-pointer hover:bg-raised/40": !!props.onClick && !props.disabled,
        "opacity-50": !!props.disabled,
      }}
      onClick={() => !props.disabled && props.onClick?.()}
    >
      <div class="min-w-0 flex-1">
        <div class="text-[0.82rem] font-medium text-ink">{props.title}</div>
        <div class="mt-0.5 text-[0.72rem] leading-relaxed text-ink-faint">{props.description}</div>
      </div>
      <div class="shrink-0 self-end sm:self-auto">{props.children}</div>
    </div>
  )
}

function ThemeRow(props: { name: ThemeName }) {
  const meta = themeMeta[props.name]
  const active = () => theme() === props.name
  const swatch = () =>
    props.name === "drift-custom"
      ? ([customTheme().background, customTheme().surface, customTheme().accent] as [string, string, string])
      : meta.swatch
  return (
    <button
      class="flex w-full items-center gap-3 rounded-lg border px-3 py-2 text-left transition-colors"
      classList={{
        "border-edge-strong bg-raised": active(),
        "border-transparent hover:bg-raised/60": !active(),
      }}
      onClick={() => setTheme(props.name)}
    >
      <span class="flex items-center">
        <For each={swatch()}>
          {(color, index) => (
            <span
              class="-ml-1.5 size-4 rounded-full border border-black/30 first:ml-0"
              style={{ background: color, "z-index": 3 - index() }}
            />
          )}
        </For>
      </span>
      <span class="min-w-0 flex-1 truncate text-sm" classList={{ "text-ink": active(), "text-ink-muted": !active() }}>
        {t(meta.label)}
      </span>
      <Show when={active()}>
        <IconCheck class="size-4 shrink-0 text-accent" />
      </Show>
    </button>
  )
}
