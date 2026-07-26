import { createEffect, onCleanup, onMount, untrack } from "solid-js"
import { EngineProvider, useEngine } from "./engine"
import { PluginHost } from "./plugins"
import { bindCodePreferences } from "./state/code"
import { initKeybinds } from "./state/keybinds"
import { bindLanguage } from "./state/language"
import { mcpCoordinator } from "./state/mcp"
import { driftStore } from "./state/store"
import { bindTheme } from "./state/theme"
import { initZoom } from "./state/zoom"
import {
  activeWorkspace,
  initWorkspaces,
  confirmDeletions,
  prepareDeletions,
  purgeAge,
  stageWorkspaceDeletion,
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
  const event = (globalThis as {
    __TAURI__?: { event?: { listen: (name: string, handler: () => void) => Promise<() => void> } }
  }).__TAURI__?.event
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

function WorkspaceBinding() {
  const engine = useEngine()
  let purging = false
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
  const timer = setInterval(() => purge(), 60 * 60 * 1000)
  // Global /global/event covers live asks; this recovers asks raised while offline.
  // Active workspace every 10s; other workspaces every 60s so instance boots stay rare.
  const permissionTimer = setInterval(() => refreshPermissions(), 10000)
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
    if (permissionTick % 6 === 0) {
      void engine.actions.refreshPermissions(paths)
      return
    }
    if (active) void engine.actions.refreshPermissions([active])
  }

  function purge() {
    if (engine.state.connection !== "online" || purging) return
    purging = true
    void (async () => {
      let sweep = await prepareDeletions(Date.now() - purgeAge)
      for (const workspace of sweep.workspaces) {
        const ids = await engine.actions.sessionIdsAt(workspace.path)
        if (!ids) continue
        sweep = { ...sweep, pending: await stageWorkspaceDeletion(workspace.id, ids) }
      }
      const confirmed = await engine.actions.removePendingSessions(sweep.pending)
      await confirmDeletions(confirmed)
    })()
      .catch(() => undefined)
      .finally(() => (purging = false))
  }
}
