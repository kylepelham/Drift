import { createEffect, onCleanup, onMount } from "solid-js"
import { EngineProvider, useEngine } from "./engine"
import { PluginHost } from "./plugins"
import { initKeybinds } from "./state/keybinds"
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
import { Chat } from "./ui/chat"
import { Composer } from "./ui/composer"
import { DebugPanel } from "./ui/debug"
import { ChatHeader } from "./ui/header"
import { Lightbox } from "./ui/lightbox"
import { McpServersModal } from "./ui/mcp"
import { AttentionNotifier } from "./ui/notifications"
import { PaletteHost } from "./ui/palette"
import { SettingsHost } from "./ui/settings"
import { Sidebar } from "./ui/sidebar"
import { Titlebar } from "./ui/titlebar"
import { ToolContextMenuHost } from "./ui/tool-context-menu"

export function App() {
  bindTheme()
  initKeybinds()
  initZoom()
  return (
    <EngineProvider>
      <WorkspaceBinding />
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
              <div class="composer-dock shrink-0 px-4 pb-4">
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
      </div>
    </EngineProvider>
  )
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

function WorkspaceBinding() {
  const engine = useEngine()
  let lastPurge = 0
  onMount(() => void initWorkspaces())
  createEffect(() => engine.setDirectory(activeWorkspace()?.path ?? null))
  createEffect(() => {
    if (engine.state.connection !== "online") return
    for (const workspace of workspaces()) void engine.actions.loadSessions(workspace.path)
    refreshPermissions()
    purge()
  })
  const timer = setInterval(() => purge(), 60 * 60 * 1000)
  // ponytail: 10s poll; other workspaces have no event stream, this is how their asks surface
  const permissionTimer = setInterval(() => refreshPermissions(), 10000)
  onCleanup(() => {
    clearInterval(timer)
    clearInterval(permissionTimer)
  })
  return null

  function refreshPermissions() {
    if (engine.state.connection !== "online") return
    void engine.actions.refreshPermissions(workspaces().map((workspace) => workspace.path))
  }

  function purge() {
    if (engine.state.connection !== "online" || Date.now() - lastPurge < dayMs) return
    lastPurge = Date.now()
    void purgeArchived().then((ids) => ids.forEach((id) => void engine.actions.remove(id)))
    void purgeRemovedWorkspaces().then((paths) => paths.forEach((path) => void engine.actions.removeAllSessions(path)))
  }
}
