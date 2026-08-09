import type { Agent, ProviderAuthMethod } from "@opencode-ai/sdk/client"
import { createEffect, createMemo, createSignal, For, Match, onCleanup, onMount, Show, Switch } from "solid-js"
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
import { formatModelContext, lmStudioMinimumContext, lmStudioModelReady } from "../state/lm-studio"
import {
  alertSounds,
  animateResponses,
  attentionKinds,
  autoAcceptGlobal,
  autoUpdate,
  collapseCompaction,
  compactionCollapsed,
  customSound,
  responseAnimationSpeed,
  responseAnimationSpeedMax,
  responseAnimationSpeedMin,
  setAlertSound,
  setAnimateResponses,
  setAutoAcceptGlobal,
  setAutoUpdate,
  setCollapseCompaction,
  setCompactionCollapsed,
  setCustomSound,
  setResponseAnimationSpeed,
  setShowReasoning,
  setShellTimeoutMs,
  listenShellTimeoutError,
  setSystemNotification,
  setToolErrorsExpanded,
  shellTimeoutMaxMs,
  shellTimeoutMinMs,
  shellTimeoutMs,
  shellTimeoutPresets,
  showReasoning,
  systemNotifications,
  toolErrorsExpanded,
  type AttentionKind,
} from "../state/prefs"
import { shellInvoke } from "../shell"
import { isRemoteRuntime } from "../runtime"
import { parseNavigationHash, pushRemoteOverlay } from "../state/navigation"
import {
  refreshRemoteAccess,
  remoteAccessBusy,
  remoteAccessError,
  remoteAccessStatus,
  nextRemoteAccessEnabled,
  remoteStatusTone,
  rotateRemoteAccessToken,
  setRemoteAccess,
} from "../state/remote-access"
import {
  setSplashDuration,
  setSplashEnabled,
  setSplashExitAnimation,
  setSplashFont,
  setSplashMascotAnimation,
  splashDuration,
  splashDurations,
  splashEnabled,
  splashExitAnimation,
  splashExitAnimations,
  splashFont,
  splashMascotAnimation,
  splashMascotAnimations,
  type SplashExitAnimation,
  type SplashMascotAnimation,
} from "../state/startup"
import {
  agentOverrideValue,
  loadPromptSnapshot,
  resetPromptOverride,
  savePromptOverride,
  type PromptOverride,
  type PromptSnapshot,
} from "../state/prompts"
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
  IconArchive,
  IconBell,
  IconCheck,
  IconChip,
  IconCode,
  IconInfo,
  IconKeyboard,
  IconMic,
  IconPalette,
  IconPlus,
  IconShieldCheck,
  IconSliders,
  IconX,
} from "./icons"
import { readDataUrl } from "./files"
import { Jellyfish } from "./jellyfish"
import { SettingsGroup, SettingsRow } from "./settings-controls"
import { StorageSection } from "./settings-storage"
import { VoiceSection } from "./settings-voice"
import { activateModal, closeOnBackdropPointerDown } from "./modal"
import { McpManagement } from "./mcp"
import { Toggle } from "./controls"
import { ProviderIcon } from "./provider-icon"
import { Picker } from "./picker"
import { Chevron } from "./controls"
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

const sections = ["General", "Appearance", "Code", "Notifications", "Voice", "Shortcuts", "Tools", "Providers", "MCP", "Prompts", "Agents", "Storage", "Remote Access", "About"] as const
type Section = (typeof sections)[number]
const sectionLabels: Record<Section, string> = {
  General: "settings.tab.general",
  Appearance: "settings.general.section.appearance",
  Code: "drift.settings.code",
  Notifications: "drift.settings.notifications",
  Voice: "drift.voice",
  Shortcuts: "settings.tab.shortcuts",
  Tools: "drift.settings.toolExecution",
  Providers: "settings.providers.title",
  MCP: "dialog.mcp.title",
  Prompts: "drift.settings.prompts",
  Agents: "settings.agents.title",
  Storage: "drift.storage",
  "Remote Access": "drift.remote.title",
  About: "drift.settings.about",
}
const sectionGroups: { label: string; items: Section[] }[] = [
  { label: "settings.section.desktop", items: ["General", "Appearance", "Code", "Notifications", "Voice", "Shortcuts"] },
  { label: "settings.section.server", items: ["Tools", "Providers", "MCP", "Prompts", "Agents"] },
  { label: "drift.settings.section", items: ["Storage", "Remote Access", "About"] },
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
const [updateSupported, setUpdateSupported] = createSignal<boolean | undefined>()
let updateSupportLoad: Promise<void> | undefined

function preloadUpdateSupport() {
  if (updateSupportLoad) return
  const invoke = shellInvoke()
  if (!invoke) return setUpdateSupported(false)
  updateSupportLoad = invoke<boolean>("update_support")
    .then((supported) => {
      setUpdateSupported(supported === true)
    })
    .catch(() => {
      setUpdateSupported(false)
    })
}

export function openSettings(section?: Section) {
  setSettingsSection(section && sections.includes(section) ? section : "General")
  if (!settingsOpen()) pushRemoteOverlay("settings")
  setSettingsOpen(true)
}

export function SettingsHost() {
  onMount(preloadUpdateSupport)
  onMount(() => {
    const sync = () => {
      if (isRemoteRuntime() && parseNavigationHash(window.location.hash).overlay !== "settings") setSettingsOpen(false)
    }
    window.addEventListener("popstate", sync)
    onCleanup(() => window.removeEventListener("popstate", sync))
  })
  const close = () => {
    setSettingsOpen(false)
    if (isRemoteRuntime() && parseNavigationHash(window.location.hash).overlay === "settings") history.back()
  }
  return (
    <Show when={settingsOpen()}>
      <Portal>
        <SettingsModal onClose={close} />
      </Portal>
    </Show>
  )
}

function SettingsModal(props: { onClose: () => void }) {
  let dialog!: HTMLDivElement
  const section = settingsSection
  const [contentScrolled, setContentScrolled] = createSignal(false)
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
        <nav class="flex w-13 shrink-0 flex-col overflow-y-auto border-r border-edge px-1.5 py-3 sm:w-44 sm:px-3">
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
        <div class="flex min-w-0 flex-1 flex-col overflow-hidden">
          <div
            class="settings-header z-10 flex items-center justify-between px-5 py-3.5"
            classList={{ "settings-header-scrolled": contentScrolled() }}
          >
            <span class="min-w-0 truncate text-sm font-semibold text-ink">{t(sectionLabels[section()])}</span>
            <button
              title={t("common.close")}
              class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
              onClick={props.onClose}
            >
              <IconX />
            </button>
          </div>
          <div
            class="min-h-0 flex-1 overflow-y-auto px-3 py-4 sm:px-4"
            onScroll={(event) => setContentScrolled(event.currentTarget.scrollTop > 1)}
          >
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
              <Match when={section() === "Voice"}>
                <VoiceSection />
              </Match>
              <Match when={section() === "Tools"}>
                <ToolExecutionSection />
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
              <Match when={section() === "Prompts"}>
                <PromptEditorSection view="prompts" />
              </Match>
              <Match when={section() === "Agents"}>
                <PromptEditorSection view="agents" />
              </Match>
              <Match when={section() === "Storage"}>
                <StorageSection />
              </Match>
              <Match when={section() === "Remote Access"}>
                <RemoteAccessSection />
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
        <SettingsRow
          title={t("drift.settings.responseAnimation.title")}
          description={t("drift.settings.responseAnimation.description")}
          onClick={() => setAnimateResponses(!animateResponses())}
        >
          <Toggle
            label={t("drift.settings.responseAnimation.title")}
            checked={animateResponses()}
            onChange={() => setAnimateResponses(!animateResponses())}
          />
        </SettingsRow>
        <Show when={animateResponses()}>
          <SettingsRow
            title={t("drift.settings.responseAnimation.speed.title")}
            description={t("drift.settings.responseAnimation.speed.description")}
          >
            <div class="flex items-center gap-2.5">
              <input
                id="response-reveal-speed"
                type="range"
                class="response-speed-slider"
                min={responseAnimationSpeedMin}
                max={responseAnimationSpeedMax}
                step="12"
                value={responseAnimationSpeed()}
                aria-label={t("drift.settings.responseAnimation.speed.title")}
                aria-valuetext={t("drift.settings.responseAnimation.speed.value", { speed: responseAnimationSpeed() })}
                onInput={(event) => setResponseAnimationSpeed(event.currentTarget.valueAsNumber)}
              />
              <output
                for="response-reveal-speed"
                class="w-7 text-right text-[0.68rem] tabular-nums text-ink-faint"
                title={t("drift.settings.responseAnimation.speed.value", { speed: responseAnimationSpeed() })}
              >
                {responseAnimationSpeed()}
              </output>
            </div>
          </SettingsRow>
        </Show>
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

      <Show when={!isRemoteRuntime()}>
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
      </Show>
    </div>
  )
}

function RemoteAccessSection() {
  const remote = isRemoteRuntime()
  const status = remoteAccessStatus
  const [copied, setCopied] = createSignal(false)
  const [rotated, setRotated] = createSignal(false)
  const [clipboardError, setClipboardError] = createSignal("")
  onMount(() => !remote && void refreshRemoteAccess())

  async function copyConnectionUrl() {
    const url = status()?.connectionUrls[0]
    if (!url) return
    setClipboardError("")
    try {
      await navigator.clipboard.writeText(url)
      setCopied(true)
      setTimeout(() => setCopied(false), 1800)
    } catch (cause) {
      setClipboardError(cause instanceof Error ? cause.message : String(cause))
    }
  }

  async function rotate() {
    setRotated(false)
    await rotateRemoteAccessToken()
    if (!remoteAccessError()) {
      setRotated(true)
      setTimeout(() => setRotated(false), 2400)
    }
  }

  const statusLabel = () =>
    status()?.error
      ? t("drift.remote.statusError")
      : status()?.listening
        ? t("drift.remote.listening")
        : status()?.enabled
          ? t("drift.remote.statusStarting")
          : t("drift.remote.statusOff")

  return (
    <div class="space-y-5">
      <Show
        when={!remote}
        fallback={
          <SettingsGroup title={t("drift.remote.gateway")}>
            <SettingsRow title={t("drift.remote.connected")} description={t("drift.remote.manageOnDesktop")}>
              <span class="size-2 rounded-full bg-ok" />
            </SettingsRow>
          </SettingsGroup>
        }
      >
        <SettingsGroup title={t("drift.remote.gateway")}>
          <SettingsRow title={t("drift.remote.enable")} description={t("drift.remote.enableDescription")}>
            <Toggle
              label={t("drift.remote.enable")}
              checked={!!status()?.enabled}
              disabled={remoteAccessBusy()}
              onChange={() => void setRemoteAccess(nextRemoteAccessEnabled(status()))}
            />
          </SettingsRow>
          <SettingsRow title={t("drift.remote.address")} description={statusLabel()}>
            <div class="flex items-center gap-2">
              <span
                class="size-2 rounded-full"
                classList={{
                  "bg-ink-faint": remoteStatusTone(status()) === "idle" || remoteStatusTone(status()) === "offline",
                  "bg-warn": remoteStatusTone(status()) === "offline" && !!status()?.enabled,
                  "bg-ok": remoteStatusTone(status()) === "online",
                  "bg-danger": remoteStatusTone(status()) === "error",
                }}
              />
              <span class="font-mono text-[0.75rem] text-ink-muted">{status()?.listeningAddress ?? "—"}</span>
            </div>
          </SettingsRow>
          <Show when={status()?.enabled && status()?.listening}>
            <SettingsRow title={t("drift.remote.connectionUrl")} description={status()?.urls[0] || t("drift.remote.noLanAddress")}>
              <div class="flex flex-wrap justify-end gap-2">
                <button
                  class="rounded-md border border-edge px-3 py-1.5 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink"
                  onClick={() => void copyConnectionUrl()}
                >
                  {copied() ? t("drift.remote.copied") : t("drift.remote.copy")}
                </button>
                <button
                  class="rounded-md border border-edge px-3 py-1.5 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40"
                  disabled={remoteAccessBusy()}
                  onClick={() => void rotate()}
                >
                  {t("drift.remote.rotate")}
                </button>
              </div>
            </SettingsRow>
          </Show>
        </SettingsGroup>
        <Show when={rotated()}><div class="text-xs text-ok">{t("drift.remote.rotated")}</div></Show>
        <Show when={clipboardError()}><div class="text-xs text-danger">{t("drift.remote.clipboardError")}: {clipboardError()}</div></Show>
      </Show>

      <Show when={remoteAccessError() || status()?.error}>
        <div class="text-xs text-danger">{remoteAccessError() || status()?.error}</div>
      </Show>
      <p class="text-[0.72rem] leading-relaxed text-ink-faint">{t("drift.remote.securityWarning")}</p>
      <p class="text-[0.72rem] leading-relaxed text-ink-faint">{t("drift.remote.deckHelp")}</p>
    </div>
  )
}

function ToolExecutionSection() {
  const isPreset = (value: number | null) => value === null || (shellTimeoutPresets as readonly number[]).includes(value)
  const [customOpen, setCustomOpen] = createSignal(!isPreset(shellTimeoutMs()))
  const [customMinutes, setCustomMinutes] = createSignal(
    String(isPreset(shellTimeoutMs()) ? 10 : shellTimeoutMs()! / 60_000),
  )
  const [error, setError] = createSignal("")
  onMount(() => {
    const stop = listenShellTimeoutError(setError)
    onCleanup(stop)
  })
  const customValue = () => Number(customMinutes()) * 60_000
  const customValid = () =>
    Number.isInteger(Number(customMinutes())) &&
    customValue() >= shellTimeoutMinMs &&
    customValue() <= shellTimeoutMaxMs

  async function applyTimeout(value: number | null) {
    setError("")
    await setShellTimeoutMs(value).catch((cause) => setError(cause instanceof Error ? cause.message : String(cause)))
  }

  const selected = () => (customOpen() || !isPreset(shellTimeoutMs()) ? "custom" : String(shellTimeoutMs()))

  return (
    <div class="space-y-5">
      <SettingsGroup title={t("drift.settings.toolExecution")}>
        <SettingsRow
          title={t("drift.settings.shellTimeout.title")}
          description={t("drift.settings.shellTimeout.description")}
        >
          <Picker
            label={t("drift.settings.shellTimeout.title")}
            items={[
              { id: "null", label: t("drift.settings.shellTimeout.noTimeout") },
              ...shellTimeoutPresets.map((value) => ({
                id: String(value),
                label: t(`drift.settings.shellTimeout.preset${value / 60_000}`),
              })),
              { id: "custom", label: t("drift.settings.shellTimeout.custom") },
            ]}
            selected={selected()}
            floating
            bordered
            chevronAtEnd
            placement="below"
            width="12rem"
            onPick={(id) => {
              if (id === "custom") return setCustomOpen(true)
              setCustomOpen(false)
              void applyTimeout(id === "null" ? null : Number(id))
            }}
          />
        </SettingsRow>
        <Show when={customOpen()}>
          <SettingsRow
            title={t("drift.settings.shellTimeout.customMinutes")}
            description={customValid() ? t("drift.settings.shellTimeout.customDescription") : t("drift.settings.shellTimeout.invalid")}
          >
            <div class="flex items-center gap-2">
                   <input
                type="number"
                min={shellTimeoutMinMs / 60_000}
                max={shellTimeoutMaxMs / 60_000}
                step="1"
                aria-label={t("drift.settings.shellTimeout.customMinutes")}
                aria-invalid={!customValid()}
                class="w-24 rounded-md border border-edge bg-surface px-2.5 py-1.5 text-right font-mono text-xs text-ink outline-none focus:border-edge-strong"
                value={customMinutes()}
                onInput={(event) => {
                  setCustomMinutes(event.currentTarget.value)
                }}
              />
              <button
                class="rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-white transition-opacity hover:opacity-90 disabled:opacity-40"
                disabled={!customValid()}
                onClick={() => void applyTimeout(customValue())}
              >
                {t("common.save")}
              </button>
            </div>
          </SettingsRow>
        </Show>
      </SettingsGroup>
      <p class="text-[0.72rem] leading-relaxed text-ink-faint">{t("drift.settings.shellTimeout.scope")}</p>
      <Show when={error()}><div class="text-xs text-danger">{error()}</div></Show>
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

  const row = (provider: (typeof engine.state.providers)[number]) => {
    const connected = () => engine.state.connected.includes(provider.id)
    const open = () => expanded() === provider.id
    return (
      <div
        class="overflow-hidden rounded-xl border transition-colors"
        classList={{
          "border-edge bg-raised/25": open(),
          "border-transparent": !open(),
        }}
      >
        <button
          type="button"
          class="group flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left transition-colors hover:bg-raised/60"
          aria-expanded={open()}
          onClick={() => setExpanded(open() ? null : provider.id)}
        >
          <span class="flex size-8 shrink-0 items-center justify-center rounded-lg border border-edge bg-surface text-ink-muted shadow-sm shadow-black/10">
            <ProviderIcon id={provider.id} class="size-4.5" />
          </span>
          <span class="min-w-0 flex-1 truncate text-sm font-medium text-ink">{provider.name}</span>
          <span
            class="inline-flex shrink-0 items-center gap-1.5 rounded-full border px-2 py-0.5 text-[0.68rem]"
            classList={{
              "border-ok/25 bg-ok/10 text-ok": connected(),
              "border-edge bg-surface/60 text-ink-faint": !connected(),
            }}
          >
            <span class="size-1.5 rounded-full" classList={{ "bg-ok": connected(), "bg-ink-faint": !connected() }} />
            {connected() ? t("mcp.status.connected") : t("drift.settings.providers.notConnected")}
          </span>
          <span class="text-ink-faint transition-colors group-hover:text-ink-muted">
            <Chevron open={open()} />
          </span>
        </button>
        <Show when={open()}>
          {provider.id === "lmstudio" ? (
            <LmStudioConnect providerName={provider.name} onNotice={setNotice} />
          ) : (
            <ProviderConnect
              providerId={provider.id}
              providerName={provider.name}
              connected={connected()}
              methods={methods()[provider.id] ?? [{ type: "api", label: t("provider.connect.method.apiKey") }]}
              onNotice={setNotice}
            />
          )}
        </Show>
      </div>
    )
  }

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

function LmStudioConnect(props: { providerName: string; onNotice: (notice: ProviderNotice) => void }) {
  const engine = useEngine()
  const [pending, setPending] = createSignal<"refresh" | "token" | null>(null)
  const [key, setKey] = createSignal("")
  const connected = () => engine.state.connected.includes("lmstudio")
  const models = () => Object.values(engine.state.providers.find((item) => item.id === "lmstudio")?.models ?? {})
  const ready = () => models().filter(lmStudioModelReady)

  async function refresh() {
    setPending("refresh")
    const ok = await engine.actions.reloadProviders()
    setPending(null)
    if (!ok) {
      props.onNotice({ tone: "error", text: t("drift.lmStudio.refreshFailed") })
      return
    }
    props.onNotice({
      tone: connected() ? "success" : "warning",
      text: connected()
        ? t("drift.lmStudio.refreshed", { count: ready().length })
        : t("drift.lmStudio.unavailable"),
    })
  }

  async function saveToken() {
    if (!key().trim()) return
    setPending("token")
    const result = await engine.actions.setProviderKey("lmstudio", key().trim())
    setPending(null)
    if (!result.ok) {
      props.onNotice({ tone: "error", text: t("drift.provider.connectFailed", { provider: props.providerName }) })
      return
    }
    setKey("")
    props.onNotice({
      tone: result.connected ? "success" : "warning",
      text: result.connected
        ? t("drift.lmStudio.refreshed", { count: ready().length })
        : t("drift.provider.savedUnavailable", { provider: props.providerName }),
    })
  }

  return (
    <div class="mx-3 mb-3 space-y-3 rounded-lg border border-edge bg-surface/55 p-3 shadow-sm shadow-black/5">
      <div class="space-y-1 text-xs text-ink-muted">
        <p>{t("drift.lmStudio.description")}</p>
        <code class="block rounded-md border border-edge bg-overlay/50 px-2 py-1.5 text-[0.68rem] text-ink-faint">
          http://127.0.0.1:1234
        </code>
      </div>
      <Show
        when={connected()}
        fallback={<div class="rounded-md border border-warn/30 bg-warn/10 px-2.5 py-2 text-xs text-warn">{t("drift.lmStudio.unavailable")}</div>}
      >
        <div class="flex items-center justify-between text-xs">
          <span class="text-ink-muted">{t("drift.lmStudio.discovered", { count: models().length })}</span>
          <span class="rounded-full border border-ok/25 bg-ok/10 px-2 py-0.5 text-ok">
            {t("drift.lmStudio.ready", { count: ready().length })}
          </span>
        </div>
        <div class="max-h-52 space-y-1 overflow-y-auto">
          <For each={models()}>
            {(model) => {
              const usable = () => lmStudioModelReady(model)
              const lowContext = () => model.capabilities.toolcall && model.limit.context < lmStudioMinimumContext
              return (
                <div class="flex items-center gap-2 rounded-md border border-edge bg-overlay/35 px-2.5 py-2">
                  <span class="min-w-0 flex-1">
                    <span class="block truncate text-xs text-ink">{model.name}</span>
                    <span class="block truncate text-[0.65rem] text-ink-faint">{model.id}</span>
                  </span>
                  <span class="shrink-0 text-[0.65rem] tabular-nums text-ink-faint">
                    {formatModelContext(model.limit.context)}
                  </span>
                  <span
                    class="shrink-0 rounded-full border px-1.5 py-0.5 text-[0.62rem]"
                    classList={{
                      "border-ok/25 bg-ok/10 text-ok": usable(),
                      "border-warn/25 bg-warn/10 text-warn": lowContext(),
                      "border-edge text-ink-faint": !usable() && !lowContext(),
                    }}
                  >
                    {usable()
                      ? t("drift.lmStudio.modelReady")
                      : lowContext()
                        ? t("drift.lmStudio.contextTooSmall")
                        : t("drift.lmStudio.notLoaded")}
                  </span>
                </div>
              )
            }}
          </For>
        </div>
        <Show when={ready().length === 0}>
          <div class="rounded-md border border-warn/30 bg-warn/10 px-2.5 py-2 text-xs text-warn">
            {t("drift.lmStudio.noReady")}
          </div>
        </Show>
      </Show>
      <div class="flex flex-wrap gap-2">
        <button
          class="h-9 rounded-md bg-accent px-3.5 text-xs font-medium text-accent-ink transition-colors hover:brightness-105 disabled:opacity-40"
          disabled={pending() !== null}
          onClick={() => void refresh()}
        >
          {pending() === "refresh" ? t("common.loading") : t("drift.lmStudio.refresh")}
        </button>
        <input
          type="password"
          class="h-9 min-w-44 flex-1 rounded-md border border-edge bg-overlay/50 px-2.5 text-sm outline-none transition-colors focus:border-edge-strong"
          placeholder={t("drift.lmStudio.apiToken")}
          value={key()}
          onInput={(event) => setKey(event.currentTarget.value)}
          onKeyDown={(event) => event.key === "Enter" && void saveToken()}
        />
        <button
          class="h-9 rounded-md border border-edge px-3 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40"
          disabled={pending() !== null || !key().trim()}
          onClick={() => void saveToken()}
        >
          {pending() === "token" ? t("common.loading") : t("common.save")}
        </button>
      </div>
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
    <div class="mx-3 mb-3 space-y-3 rounded-lg border border-edge bg-surface/55 p-3 shadow-sm shadow-black/5">
      <Show when={props.methods.length > 1}>
        <div class="flex flex-wrap gap-1 rounded-lg border border-edge bg-overlay/50 p-1">
          <For each={props.methods}>
            {(item, index) => (
              <button
                class="rounded-md px-2.5 py-1 text-xs transition-colors"
                classList={{
                  "bg-raised text-ink shadow-sm shadow-black/10": index() === methodIndex(),
                  "text-ink-faint hover:bg-raised/60 hover:text-ink-muted": index() !== methodIndex(),
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
            class="h-9 min-w-0 flex-1 rounded-md border border-edge bg-overlay/50 px-2.5 text-sm outline-none transition-colors focus:border-edge-strong"
            placeholder={t("provider.connect.apiKey.placeholder")}
            value={key()}
            onInput={(event) => setKey(event.currentTarget.value)}
            onKeyDown={(event) => event.key === "Enter" && void connectApi()}
          />
          <button
            class="h-9 rounded-md bg-accent px-3.5 text-xs font-medium text-accent-ink transition-colors hover:brightness-105 disabled:opacity-40"
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
                class="h-9 rounded-md bg-accent px-3.5 text-xs font-medium text-accent-ink transition-colors hover:brightness-105 disabled:opacity-40"
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
                    class="h-9 min-w-0 flex-1 rounded-md border border-edge bg-overlay/50 px-2.5 text-sm outline-none transition-colors focus:border-edge-strong"
                    placeholder={t("provider.connect.oauth.code.placeholder")}
                    value={code()}
                    onInput={(event) => setCode(event.currentTarget.value)}
                    onKeyDown={(event) => event.key === "Enter" && void submitCode()}
                  />
                  <button
                    class="h-9 rounded-md bg-accent px-3.5 text-xs font-medium text-accent-ink transition-colors hover:brightness-105 disabled:opacity-40"
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
        <div class="flex items-center justify-between gap-4 border-t border-edge pt-3">
          <span class="min-w-0 text-xs leading-relaxed text-ink-faint">{t("drift.provider.disconnectDescription")}</span>
          <button
            class="h-8 shrink-0 rounded-md border border-danger/40 px-3 text-xs font-medium text-danger transition-colors hover:border-danger/60 hover:bg-danger/10 disabled:opacity-40"
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

function PromptEditorSection(props: { view: "prompts" | "agents" }) {
  const engine = useEngine()
  const [snapshot, setSnapshot] = createSignal<PromptSnapshot | null>(null)
  const [familyID, setFamilyID] = createSignal("gpt")
  const [familyPrompt, setFamilyPrompt] = createSignal("")
  const [agentName, setAgentName] = createSignal("build")
  const [agentPrompt, setAgentPrompt] = createSignal("")
  const [agentBehavior, setAgentBehavior] = createSignal("{}")
  const [familyBaseline, setFamilyBaseline] = createSignal("")
  const [agentPromptBaseline, setAgentPromptBaseline] = createSignal("")
  const [agentBehaviorBaseline, setAgentBehaviorBaseline] = createSignal("{}")
  const [familyDirty, setFamilyDirty] = createSignal(false)
  // Prompt and agent overrides are read by the engine at startup, so a successful write only
  // takes effect after a restart. This flag drives that notice, nothing else.
  const [showRestartNotice, setShowRestartNotice] = createSignal(false)
  const [error, setError] = createSignal("")
  const [saving, setSaving] = createSignal(false)
  const override = (key: string) => snapshot()?.overrides.find((item) => item.key === key)
  const familyOverridden = () => !!override(`family:${familyID()}`)
  const agentOverridden = () => !!override(`agent:${agentName()}`)
  const agentDirty = () => agentPrompt() !== agentPromptBaseline() || agentBehavior() !== agentBehaviorBaseline()
  const agentOverrideFields = () => {
    const storedValue = override(`agent:${agentName()}`)?.value
    if (!storedValue || typeof storedValue !== "object" || Array.isArray(storedValue)) return {}
    return storedValue as Record<string, unknown>
  }
  const familyModified = () => familyDirty() || familyOverridden()
  const agentPromptModified = () => agentPrompt() !== agentPromptBaseline() || "prompt" in agentOverrideFields()
  const agentBehaviorModified = () =>
    agentBehavior() !== agentBehaviorBaseline() || Object.keys(agentOverrideFields()).some((key) => key !== "prompt")

  async function load() {
    const next = await loadPromptSnapshot().catch((cause) => {
      setError(cause instanceof Error ? cause.message : String(cause))
      return null
    })
    setSnapshot(next)
  }

  onMount(() => void load())

  createEffect(() => {
    if (familyDirty()) return
    const family = snapshot()?.catalog.families.find((item) => item.id === familyID())
    const value = override(`family:${familyID()}`)?.value
    const prompt = typeof value === "string" ? value : (family?.default ?? "")
    setFamilyPrompt(prompt)
    setFamilyBaseline(prompt)
  })

  // Splits a resolved agent config into the two editors: the prompt gets its own textarea, every
  // other field is edited as raw JSON. Both editors reset their baseline so nothing reads as dirty.
  function loadAgentEditors(config: ReturnType<typeof agentConfig>) {
    const { prompt: promptField, ...behavior } = config
    const prompt = typeof promptField === "string" ? promptField : ""
    const serialized = JSON.stringify(behavior, null, 2)
    setAgentPrompt(prompt)
    setAgentPromptBaseline(prompt)
    setAgentBehavior(serialized)
    setAgentBehaviorBaseline(serialized)
  }

  function currentAgent() {
    return engine.state.agents.find((item) => item.name === agentName())
  }

  createEffect(() => {
    if (agentDirty()) return
    const storedOverride = override(`agent:${agentName()}`)
    loadAgentEditors(agentConfig(currentAgent(), snapshot(), storedOverride))
  })

  async function mutate(action: () => Promise<void>, clean: () => void) {
    setSaving(true)
    setError("")
    setShowRestartNotice(false)
    try {
      await action()
      clean()
      await load()
      setShowRestartNotice(true)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setSaving(false)
    }
  }

  function saveAgent() {
    let behavior: unknown
    try {
      behavior = JSON.parse(agentBehavior())
    } catch {
      setError(t("drift.settings.prompts.invalidJson"))
      return
    }
    if (!behavior || typeof behavior !== "object" || Array.isArray(behavior)) {
      setError(t("drift.settings.prompts.invalidJson"))
      return
    }
    const key = `agent:${agentName()}`
    const storedOverride = override(key)
    const existing =
      storedOverride?.value && typeof storedOverride.value === "object"
        ? (storedOverride.value as Record<string, unknown>)
        : {}
    const baseline = JSON.parse(agentBehaviorBaseline()) as Record<string, unknown>
    const value = agentOverrideValue(
      { ...(behavior as object), prompt: agentPrompt() },
      { ...baseline, prompt: agentPromptBaseline() },
      existing,
    )
    const original = storedOverride?.original ?? agentConfig(currentAgent(), snapshot())
    const action = Object.keys(value).length
      ? () => savePromptOverride(key, value, original)
      : () => resetPromptOverride(key)
    void mutate(action, () => {
      setAgentPromptBaseline(agentPrompt())
      setAgentBehaviorBaseline(agentBehavior())
    })
  }

  function resetFamily() {
    const key = `family:${familyID()}`
    if (override(key)) return void mutate(() => resetPromptOverride(key), () => setFamilyDirty(false))
    const family = snapshot()?.catalog.families.find((item) => item.id === familyID())
    const prompt = family?.default ?? ""
    setFamilyPrompt(prompt)
    setFamilyBaseline(prompt)
    setFamilyDirty(false)
  }

  function resetAgent() {
    const key = `agent:${agentName()}`
    if (override(key)) {
      return void mutate(() => resetPromptOverride(key), () => {
        setAgentPromptBaseline(agentPrompt())
        setAgentBehaviorBaseline(agentBehavior())
      })
    }
    loadAgentEditors(agentConfig(currentAgent(), snapshot()))
  }

  return (
    <div class="space-y-6">
      <Show
        when={snapshot()}
        fallback={
          <Show when={!error()}>
            <div class="px-2 text-sm text-ink-faint">{t("common.loading")}</div>
          </Show>
        }
      >
        {(data) => (
          <>
            <Show when={props.view === "prompts"}>
              <SettingsGroup title={t("drift.settings.prompts.modelFamilies")}>
              <div class="space-y-3 py-3">
                <div class="flex items-center justify-between gap-3">
                  <div class="text-xs text-ink-faint">{t("drift.settings.prompts.familyDescription")}</div>
                  <Picker
                    label={t("drift.settings.prompts.modelFamilies")}
                    items={data().catalog.families.map((family) => ({ id: family.id, label: familyLabel(family.id) }))}
                    selected={familyID()}
                    floating bordered chevronAtEnd placement="below" width="11rem"
                    onPick={(value) => {
                      if (familyDirty()) {
                        setError(t("drift.settings.prompts.saveBeforeSwitch"))
                        return
                      }
                      setFamilyDirty(false)
                      setFamilyID(value)
                    }}
                  />
                </div>
                <textarea
                  aria-label={t("drift.settings.prompts.systemPrompt")}
                  class="h-64 w-full resize-y rounded-lg border border-edge bg-bg/50 p-3 font-mono text-xs leading-relaxed outline-none transition-colors focus:border-accent"
                  classList={{ "text-ink": familyModified(), "text-ink-faint": !familyModified() }}
                  spellcheck={false}
                  value={familyPrompt()}
                  onInput={(event) => {
                    const value = event.currentTarget.value
                    setFamilyPrompt(value)
                    setFamilyDirty(value !== familyBaseline())
                  }}
                />
                <details class="text-xs text-ink-faint">
                  <summary class="cursor-pointer select-none">{t("drift.settings.prompts.upstreamOriginal")}</summary>
                  <pre class="mt-2 max-h-48 overflow-auto whitespace-pre-wrap rounded-lg bg-bg/40 p-3 font-mono text-[0.68rem] leading-relaxed">
                    {data().catalog.families.find((item) => item.id === familyID())?.original}
                  </pre>
                </details>
                <PromptActions
                  disabled={saving()}
                  dirty={familyDirty()}
                  overridden={familyOverridden()}
                  onSave={() =>
                    void mutate(() => savePromptOverride(`family:${familyID()}`, familyPrompt()), () =>
                      setFamilyDirty(false),
                    )
                  }
                  onReset={resetFamily}
                />
              </div>
              </SettingsGroup>
            </Show>

            <Show when={props.view === "agents"}>
              <SettingsGroup title={t("drift.settings.prompts.agents")}>
              <div class="space-y-3 py-3">
                <div class="flex items-center justify-between gap-3">
                  <div class="text-xs text-ink-faint">{t("drift.settings.prompts.agentDescription")}</div>
                  <Picker
                    label={t("drift.settings.prompts.agents")}
                    items={engine.state.agents.map((agent) => ({ id: agent.name, label: agent.name, hint: agent.description }))}
                    selected={agentName()}
                    floating bordered chevronAtEnd placement="below" width="11rem"
                    onPick={(value) => {
                      if (agentDirty()) {
                        setError(t("drift.settings.prompts.saveBeforeSwitch"))
                        return
                      }
                      setAgentName(value)
                    }}
                  />
                </div>
                <label class="block text-xs text-ink-faint">
                  <span class="mb-1 block">{t("drift.settings.prompts.agentPrompt")}</span>
                  <textarea
                    class="h-48 w-full resize-y rounded-lg border border-edge bg-bg/50 p-3 font-mono text-xs leading-relaxed outline-none transition-colors focus:border-accent"
                    classList={{ "text-ink": agentPromptModified(), "text-ink-faint": !agentPromptModified() }}
                    spellcheck={false}
                    placeholder={t("drift.settings.prompts.inheritsFamily")}
                    value={agentPrompt()}
                    onInput={(event) => {
                      const value = event.currentTarget.value
                      setAgentPrompt(value)
                    }}
                  />
                </label>
                <label class="block text-xs text-ink-faint">
                  <span class="mb-1 block">{t("drift.settings.prompts.behavior")}</span>
                  <textarea
                    class="h-40 w-full resize-y rounded-lg border border-edge bg-bg/50 p-3 font-mono text-xs leading-relaxed outline-none transition-colors focus:border-accent"
                    classList={{ "text-ink": agentBehaviorModified(), "text-ink-faint": !agentBehaviorModified() }}
                    spellcheck={false}
                    value={agentBehavior()}
                    onInput={(event) => {
                      const value = event.currentTarget.value
                      setAgentBehavior(value)
                    }}
                  />
                </label>
                <PromptActions
                  disabled={saving()}
                  dirty={agentDirty()}
                  overridden={agentOverridden()}
                  onSave={saveAgent}
                  onReset={resetAgent}
                />
              </div>
              </SettingsGroup>
            </Show>
          </>
        )}
      </Show>
      <Show when={showRestartNotice()}>
        <div class="text-xs text-accent">{t("drift.settings.prompts.restart")}</div>
      </Show>
      <Show when={error()}>
        <div class="text-xs text-danger">{error()}</div>
      </Show>
    </div>
  )
}

function PromptActions(props: {
  disabled: boolean
  dirty: boolean
  overridden: boolean
  onSave: () => void
  onReset: () => void
}) {
  return (
    <div class="flex justify-end gap-2">
      <button
        class="rounded-md border border-edge px-3 py-1.5 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40"
        disabled={props.disabled || (!props.dirty && !props.overridden)}
        onClick={props.onReset}
      >
        {t("common.reset")}
      </button>
      <button
        class="rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-white transition-opacity hover:opacity-90 disabled:opacity-40"
        disabled={props.disabled || !props.dirty}
        onClick={props.onSave}
      >
        {t("common.save")}
      </button>
    </div>
  )
}

function familyLabel(id: string) {
  const labels: Record<string, string> = {
    meta: "Meta Muse",
    beast: "GPT-4 / o1 / o3",
    codex: "GPT Codex",
    gpt: "GPT",
    gemini: "Gemini",
    anthropic: "Claude",
    trinity: "Trinity",
    kimi: "Kimi",
    default: "Default",
  }
  return labels[id] ?? id
}

function agentConfig(agent: Agent | undefined, snapshot: PromptSnapshot | null, storedOverride?: PromptOverride) {
  const restored =
    storedOverride?.value && typeof storedOverride.value === "object"
      ? (storedOverride.value as Record<string, unknown>)
      : undefined
  if (!agent) return restored ? { ...restored } : {}
  const source = agent as Agent & {
    prompt?: string
    hidden?: boolean
    model?: { providerID: string; modelID: string }
    variant?: string
    temperature?: number
    topP?: number
    steps?: number
  }
  return {
    prompt: source.prompt ?? snapshot?.catalog.agents.find((item) => item.name === source.name)?.prompt,
    description: source.description,
    mode: source.mode,
    hidden: source.hidden,
    model: source.model ? `${source.model.providerID}/${source.model.modelID}` : undefined,
    variant: source.variant,
    temperature: source.temperature,
    top_p: source.topP,
    steps: source.steps,
    ...restored,
  }
}

const websiteUrl = "https://driftagent.dev"

function AboutSection() {
  const engine = useEngine()
  const engineVersion = () =>
    engine.state.version || (engine.state.startupError ? t("drift.about.failed") : t("drift.about.starting"))

  return (
    <div class="space-y-6 select-text">
      <div class="flex flex-col items-center gap-2 text-center">
        <Jellyfish class="size-32" />
        <div class="drift-wordmark text-base">drift</div>
        <p class="max-w-xs text-[0.76rem] leading-relaxed text-ink-muted">{t("drift.about.description")}</p>
      </div>

      <SettingsGroup title={t("drift.about.group.build")}>
        <SettingsRow title={t("drift.about.row.app.title")} description={t("drift.about.row.app.description")}>
          <span class="font-mono text-[0.75rem] text-ink-muted">{__DRIFT_VERSION__}</span>
        </SettingsRow>
        <SettingsRow title={t("drift.about.row.engine.title")} description={t("drift.about.row.engine.description")}>
          <span class="font-mono text-[0.75rem] text-ink-muted">{engineVersion()}</span>
        </SettingsRow>
        <SettingsRow
          title={t("drift.about.row.updates.title")}
          description={
            updateSupported() === undefined
              ? t("drift.about.starting")
              : updateSupported()
                ? t("drift.about.row.updates.installed")
                : t("drift.about.row.updates.local")
          }
        >
          <span class="text-[0.75rem] text-ink-muted">
            {updateSupported() === undefined
              ? t("common.loading")
              : updateSupported()
                ? t("drift.about.updates.available")
                : t("drift.about.updates.unavailable")}
          </span>
        </SettingsRow>
        <Show when={engine.state.startupError}>
          <div class="px-1 py-2.5 text-[0.72rem] leading-relaxed text-danger">{engine.state.startupError}</div>
        </Show>
      </SettingsGroup>

      <SettingsGroup title={t("drift.about.group.links")}>
        <SettingsRow title={t("drift.about.row.website.title")} description={t("drift.about.row.website.description")}>
          <button
            class="rounded border border-edge px-2 py-1 text-[0.72rem] text-accent hover:bg-raised"
            onClick={() => openExternal(websiteUrl)}
          >
            driftagent.dev
          </button>
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup title={t("drift.about.group.credits")}>
        <div class="space-y-2 px-1 py-2.5 text-[0.74rem] leading-relaxed text-ink-muted">
          <p>{t("drift.about.credits.engine")}</p>
        </div>
      </SettingsGroup>
    </div>
  )
}

const customColorMeta: { id: keyof CustomTheme; label: string }[] = [
  { id: "background", label: "drift.color.background" },
  { id: "surface", label: "drift.color.surface" },
  { id: "text", label: "drift.color.text" },
  { id: "accent", label: "drift.color.accent" },
]
const mascotAnimationLabels: Record<SplashMascotAnimation, string> = {
  bounce: "startup.settings.mascot.bounce",
  float: "startup.settings.mascot.float",
  pulse: "startup.settings.mascot.pulse",
  still: "startup.settings.mascot.still",
}
const exitAnimationLabels: Record<SplashExitAnimation, string> = {
  wave: "startup.settings.exit.wave",
  fade: "startup.settings.exit.fade",
  lift: "startup.settings.exit.lift",
}
const durationLabels: Record<number, string> = {
  1500: "startup.settings.duration.brief",
  3200: "startup.settings.duration.balanced",
  5000: "startup.settings.duration.extended",
}

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
                     maxLength={7}
                     pattern="#[0-9a-fA-F]{6}"
                     value={customTheme()[color.id]}
                     onChange={(event) => {
                       if (/^#[\da-f]{6}$/i.test(event.currentTarget.value)) setCustomThemeColor(color.id, event.currentTarget.value)
                     }}
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

      <SettingsGroup title={t("startup.settings.title")}>
        <SettingsRow
          title={t("startup.settings.show.title")}
          description={t("startup.settings.show.description")}
          onClick={() => setSplashEnabled(!splashEnabled())}
        >
          <Toggle
            label={t("startup.settings.show.title")}
            checked={splashEnabled()}
            onChange={() => setSplashEnabled(!splashEnabled())}
          />
        </SettingsRow>
        <Show when={splashEnabled()}>
          <SettingsRow
            title={t("startup.settings.mascot.title")}
            description={t("startup.settings.mascot.description")}
          >
            <Picker
              label={t("startup.settings.mascot.title")}
              items={splashMascotAnimations.map((name) => ({ id: name, label: t(mascotAnimationLabels[name]) }))}
              selected={splashMascotAnimation()}
              floating bordered chevronAtEnd placement="below" width="10rem"
              onPick={(value) => setSplashMascotAnimation(value as SplashMascotAnimation)}
            />
          </SettingsRow>
          <SettingsRow
            title={t("startup.settings.exit.title")}
            description={t("startup.settings.exit.description")}
          >
            <Picker
              label={t("startup.settings.exit.title")}
              items={splashExitAnimations.map((name) => ({ id: name, label: t(exitAnimationLabels[name]) }))}
              selected={splashExitAnimation()}
              floating bordered chevronAtEnd placement="below" width="10rem"
              onPick={(value) => setSplashExitAnimation(value as SplashExitAnimation)}
            />
          </SettingsRow>
          <SettingsRow
            title={t("startup.settings.duration.title")}
            description={t("startup.settings.duration.description")}
          >
            <Picker
              label={t("startup.settings.duration.title")}
              items={splashDurations.map((duration) => ({ id: String(duration), label: t(durationLabels[duration]) }))}
              selected={String(splashDuration())}
              floating bordered chevronAtEnd placement="below" width="11rem"
              onPick={(value) => setSplashDuration(Number(value))}
            />
          </SettingsRow>
          <SettingsRow
            title={t("startup.settings.font.title")}
            description={t("startup.settings.font.description")}
          >
            <FontField label={t("startup.settings.font.title")} value={splashFont()} onInput={setSplashFont} />
          </SettingsRow>
        </Show>
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
      maxLength={256}
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
    if (props.section === "Voice") return <IconMic />
    if (props.section === "Shortcuts") return <IconKeyboard />
    if (props.section === "Tools") return <IconSliders />
    if (props.section === "Providers") return <IconChip />
    if (props.section === "MCP") return <IconShieldCheck />
    if (props.section === "Prompts") return <IconCode />
    if (props.section === "Agents") return <IconSliders />
    if (props.section === "Storage") return <IconArchive />
    if (props.section === "Remote Access") return <IconShieldCheck />
    return <IconInfo />
  }
  return <span class="flex size-5 shrink-0 items-center justify-center text-ink-faint">{icon()}</span>
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
