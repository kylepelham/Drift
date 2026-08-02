import { createEffect, createSignal, onCleanup, onMount, Show, untrack } from "solid-js"
import { EngineProvider, useEngine } from "./engine"
import { PluginHost } from "./plugins"
import { shellEvents } from "./shell"
import { bindCodePreferences } from "./state/code"
import { runScheduledCleanup } from "./state/storage"
import { initKeybinds } from "./state/keybinds"
import { t } from "./state/i18n"
import { bindLanguage } from "./state/language"
import { mcpCoordinator } from "./state/mcp"
import { driftStore } from "./state/store"
import { initRecoverableInterruptions } from "./state/recovery"
import { bindTheme } from "./state/theme"
import { closeMobileDrawer, mobileDrawerOpen } from "./state/navigation"
import { initZoom } from "./state/zoom"
import { bindShellTimeoutPolicy } from "./state/prefs"
import { listenMirrorLiveError } from "./state/mirror"
import { activeWorkspace, initWorkspaces, purgeAll, workspaces } from "./state/workspaces"
import { AttentionStrip } from "./ui/attention"
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
  onMount(() => void syncDictationConsent().catch(() => undefined))
  return (
    <EngineProvider>
      <WorkspaceBinding />
      <McpBinding />
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
          <main class="flex min-w-0 flex-1">
            <div class="flex min-w-0 flex-1 flex-col">
              <div class="relative flex min-h-0 flex-1 flex-col">
                <ChatHeader />
                <Chat />
              </div>
              <div
                class="composer-dock shrink-0 px-4 pb-4"
                onWheel={(event) => forwardWheelToChat(event, event.currentTarget)}
              >
                <AttentionStrip />
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
      <div class="fixed right-3 bottom-3 z-100 max-w-sm rounded-md border border-danger/35 bg-surface px-3 py-2 text-xs text-danger shadow-lg">
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
    void initRecoverableInterruptions()
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
