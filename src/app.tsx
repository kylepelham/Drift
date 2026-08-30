import { createEffect, createSignal, onCleanup, onMount, Show, untrack } from "solid-js"
import { EngineProvider, useEngine } from "./engine"
import { messageText } from "./engine/store"
import { PluginHost } from "./plugins"
import { shellEvents } from "./shell"
import { bindCodePreferences } from "./state/code"
import { runScheduledCleanup } from "./state/storage"
import { initKeybinds } from "./state/keybinds"
import { t } from "./state/i18n"
import { bindLanguage } from "./state/language"
import { mcpCoordinator } from "./state/mcp"
import { driftStore } from "./state/store"
import { bindTheme } from "./state/theme"
import { closeMobileDrawer, mobileDrawerOpen } from "./state/navigation"
import { initZoom } from "./state/zoom"
import { bindShellTimeoutPolicy, prefsFor } from "./state/prefs"
import {
  isGeneratedUserEntry,
  ORCHESTRATOR_AGENT,
  orchestratorGate,
  parseOrchestratorStatus,
  PROCEED_PROMPT,
  STATUS_REMINDER_PROMPT,
} from "./state/orchestrator"
import { initDevtoolsShortcut } from "./state/devtools"
import { listenMirrorLiveError } from "./state/mirror"
import { activeWorkspace, initWorkspaces, purgeAll, workspaces } from "./state/workspaces"
import { debugPanelOpen } from "./state/panels"
import { selectedSession } from "./state/selection"
import { Chat, forwardWheelToChat } from "./ui/chat"
import { Composer } from "./ui/composer"
import { DebugPanel } from "./ui/debug"
import { ChatHeader } from "./ui/header"
import { Lightbox } from "./ui/lightbox"
import { McpServersModal } from "./ui/mcp"
import { AttentionNotifier, NoticeHost } from "./ui/notifications"
import { PaletteHost } from "./ui/palette"
import { SettingsHost } from "./ui/settings"
import { Sidebar } from "./ui/sidebar"
import { StartupSplash } from "./ui/startup"
import { Titlebar } from "./ui/titlebar"
import { ToolContextMenuHost } from "./ui/tool-context-menu"
import { syncDictationConsent } from "./voice/dictation"

export function App() {
  bindTheme()
  bindCodePreferences()
  bindLanguage()
  initKeybinds()
  initZoom()
  bindShellTimeoutPolicy()
  onCleanup(initDevtoolsShortcut())
  onMount(() => void syncDictationConsent().catch(() => undefined))
  return (
    <EngineProvider>
      <WorkspaceBinding />
      <McpBinding />
      <OrchestratorBinding />
      <PluginBinding />
      <div class="app-shell flex h-full flex-col bg-bg text-ink">
        <Titlebar />
        <div class="flex min-h-0 flex-1">
          <Show when={mobileDrawerOpen()}>
            <button
              aria-label={t("common.close")}
              class="mobile-sidebar-backdrop fixed inset-0 z-30 bg-black/55"
              onClick={() => closeMobileDrawer()}
            />
          </Show>
          <Sidebar />
          <main class="flex min-h-0 min-w-0 flex-1 overflow-hidden">
            <div
              class="chat-pane flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
              classList={{ "chat-pane-covered": debugPanelOpen() && !!selectedSession() }}
            >
              <div class="relative flex min-h-0 flex-1 flex-col">
                <ChatHeader />
                <Chat />
              </div>
              <div
                class="composer-dock shrink-0 px-4 pb-4"
                onWheel={(event) => forwardWheelToChat(event, event.currentTarget)}
              >
                <Composer />
              </div>
            </div>
            <DebugPanel />
          </main>
        </div>
        <Lightbox />
        <McpServersModal />
        <SettingsHost />
        <PaletteHost />
        <ToolContextMenuHost />
        <NoticeHost />
        <MirrorConnectionNotice />
      </div>
      <StartupSplash />
    </EngineProvider>
  )
}

function MirrorConnectionNotice() {
  const [error, setError] = createSignal("")
  onMount(() => {
    const stop = listenMirrorLiveError(setError)
    onCleanup(stop)
  })
  return (
    <Show when={error()}>
      <div class="fixed right-3 bottom-3 z-20 max-w-sm rounded-md border border-danger/35 bg-surface px-3 py-2 text-xs text-danger shadow-lg">
        {error()}
      </div>
    </Show>
  )
}

function McpBinding() {
  const engine = useEngine()
  const event = shellEvents()
  const stop = mcpCoordinator.start({
    store: driftStore,
    status: engine.actions.mcpStatus,
    connect: engine.actions.mcpConnect,
    disconnect: engine.actions.mcpDisconnect,
    authenticate: engine.actions.mcpAuthenticate,
    listen: event
      ? (refresh) => event.listen("mcp-config-changed", refresh)
      : undefined,
  })
  onCleanup(stop)
  createEffect(() => {
    void mcpCoordinator.setActive(engine.state.directory, engine.state.connection === "online").catch(() => undefined)
  })
  return null
}

function PluginBinding() {
  const engine = useEngine()
  return (
    <>
      <PluginHost engine={engine} />
      <AttentionNotifier engine={engine} />
    </>
  )
}

function OrchestratorBinding() {
  const engine = useEngine()
  const previous = new Map<string, string>()
  // Rounds are anchored to the user's goal message, so a fresh goal resets the budget while
  // Drift's own generated proceed prompts never do.
  const rounds = new Map<string, { anchor: string; count: number; capNotified?: boolean }>()
  const driving = new Set<string>()

  createEffect(() => {
    for (const [id, status] of Object.entries(engine.state.status)) {
      const before = previous.get(id)
      previous.set(id, status.type)
      if (status.type !== "idle" || driving.has(id)) continue
      // The microtask escapes the effect's tracking scope: driving reads a lot of state that
      // must not resubscribe this effect.
      if (before === "busy" || before === "retry") queueMicrotask(() => void drive(id, before))
    }
    for (const id of previous.keys()) if (!engine.state.status[id]) previous.delete(id)
  })

  async function drive(id: string, previousStatus: string) {
    const state = engine.state
    const session = state.sessions[id]
    const entries = state.transcripts[id] ?? []
    const last = entries.at(-1)
    const goal = [...entries]
      .reverse()
      .find((entry) => entry.info.role === "user" && !isGeneratedUserEntry(entry.parts as never))
    if (!session || !goal || !last) return
    const record = rounds.get(id)
    const count = record && record.anchor === goal.info.id ? record.count : 0
    const blocked = orchestratorGate({
      previousStatus,
      status: state.status[id]?.type ?? "idle",
      goalAgent: (goal.info as { agent?: string }).agent,
      parentID: session.parentID,
      pendingAsks: (state.permissions[id]?.length ?? 0) + (state.questions[id]?.length ?? 0),
      lastMessage:
        last.info.role === "assistant"
          ? {
              role: last.info.role,
              completed: !!(last.info as { time: { completed?: number } }).time.completed,
              errored: !!(last.info as { error?: unknown }).error,
            }
          : undefined,
      rounds: count,
    })
    if (blocked === "round limit reached" && !record?.capNotified) {
      rounds.set(id, { anchor: goal.info.id, count, capNotified: true })
      engine.actions.notice({
        title: "Orchestrator paused",
        message: "The round limit was reached for this goal. Send a message to keep going.",
        variant: "warning",
      })
      return
    }
    if (blocked) return
    const status = parseOrchestratorStatus(messageText(last))
    if (status?.state === "done") {
      engine.actions.notice({
        title: "Orchestrator finished",
        message: status.headline ?? "The goal was reported complete.",
        variant: "success",
      })
      return
    }
    if (status?.state === "blocked") {
      engine.actions.notice({
        title: "Orchestrator blocked",
        message: status.headline ?? "The orchestrator needs your input to continue.",
        variant: "warning",
      })
      return
    }
    driving.add(id)
    try {
      rounds.set(id, { anchor: goal.info.id, count: count + 1 })
      const prefs = prefsFor(id)
      const result = await engine.actions.steer(
        id,
        status?.state === "working" ? PROCEED_PROMPT : STATUS_REMINDER_PROMPT,
        { model: prefs.model, agent: ORCHESTRATOR_AGENT, ...(prefs.variant ? { variant: prefs.variant } : {}) },
      )
      // A rejected steer leaves the session idle, so no later transition would restart the driver.
      if (!result.ok)
        engine.actions.notice({ title: "Orchestrator paused", message: result.error, variant: "warning" })
    } finally {
      driving.delete(id)
    }
  }

  return null
}

const dayMs = 24 * 60 * 60 * 1000
const purgeIntervalMs = 60 * 60 * 1000
// The active workspace is polled on every tick; every other workspace is polled less often because
// each sweep may have to boot an engine instance for a directory that is not currently loaded.
const activePermissionPollMs = 10_000
const allWorkspacePermissionPollMs = 60_000
const ticksPerAllWorkspaceSweep = allWorkspacePermissionPollMs / activePermissionPollMs

function WorkspaceBinding() {
  const engine = useEngine()
  let lastPurge = 0
  let permissionTick = 0
  onMount(() => {
    void initWorkspaces()
  })
  createEffect(() => engine.setDirectory(activeWorkspace()?.path ?? null))
  createEffect(() => {
    if (engine.state.connection !== "online") return
    void engine.actions.loadAllSessions()
    // Full sweep once on connect; the timer keeps the active workspace hot afterward.
    const paths = untrack(() => workspaces().map((workspace) => workspace.path))
    void engine.actions.refreshPermissions(paths)
    purge()
  })
  const timer = setInterval(() => purge(), purgeIntervalMs)
  // Global /global/event covers live asks; this recovers asks raised while offline.
  const permissionTimer = setInterval(() => refreshPermissions(), activePermissionPollMs)
  onCleanup(() => {
    clearInterval(timer)
    clearInterval(permissionTimer)
  })
  return null

  function refreshPermissions() {
    if (engine.state.connection !== "online") return
    const active = activeWorkspace()?.path
    const paths = workspaces().map((workspace) => workspace.path)
    permissionTick += 1
    if (permissionTick % ticksPerAllWorkspaceSweep === 0) {
      void engine.actions.refreshPermissions(paths)
      return
    }
    if (active) void engine.actions.refreshPermissions([active])
  }

  function purge() {
    if (engine.state.connection !== "online" || Date.now() - lastPurge < dayMs) return
    lastPurge = Date.now()
    void purgeAll(engine.actions).then((complete) => {
      // Failed engine deletions kept their tombstones; clearing the stamp retries on the next
      // reconnect or hourly tick instead of waiting out the daily interval.
      if (!complete) lastPurge = 0
    })
    // Storage cleanup rides the same daily timer and keeps its own last-run stamp, so it stays off
    // the startup path where a large event log would block the first paint.
    void runScheduledCleanup().catch(() => undefined)
  }
}
