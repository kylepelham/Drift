import { createEffect, onCleanup, onMount, untrack } from "solid-js"
import { EngineProvider, useEngine } from "./engine"
import { PluginHost } from "./plugins"
import { shellEvents } from "./shell"
import { bindCodePreferences } from "./state/code"
import { runScheduledCleanup } from "./state/storage"
import { initKeybinds } from "./state/keybinds"
import { bindLanguage } from "./state/language"
import { mcpCoordinator } from "./state/mcp"
import { driftStore } from "./state/store"
import { bindTheme } from "./state/theme"
import { initZoom } from "./state/zoom"
import {
  activeWorkspace,
  initWorkspaces,
  purgeArchived,
  purgeRemovedWorkspaces,
  workspaces,
} from "./state/workspaces"
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

export function App() {
  bindTheme()
  bindCodePreferences()
  bindLanguage()
  initKeybinds()
  initZoom()
  return (
    <EngineProvider>
      <WorkspaceBinding />
      <McpBinding />
      <PluginBinding />
      <div class="flex h-full flex-col bg-bg text-ink">
        <Titlebar />
        <div class="flex min-h-0 flex-1">
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
      </div>
      <StartupSplash />
    </EngineProvider>
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
  onMount(() => void initWorkspaces())
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
    void purgeArchived().then((ids) => ids.forEach((id) => void engine.actions.remove(id)))
    void purgeRemovedWorkspaces((directory, eligible) => engine.actions.removeAllSessions(directory, eligible))
    // Storage cleanup rides the same daily timer and keeps its own last-run stamp, so it stays off
    // the startup path where a large event log would block the first paint.
    void runScheduledCleanup().catch(() => undefined)
  }
}
