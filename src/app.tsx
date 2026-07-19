import { createEffect, onCleanup, onMount } from "solid-js"
import { EngineProvider, useEngine } from "./engine"
import { PluginHost } from "./plugins"
import { bindTheme } from "./state/theme"
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
import { ChatHeader } from "./ui/header"
import { Sidebar } from "./ui/sidebar"
import { Titlebar } from "./ui/titlebar"

export function App() {
  bindTheme()
  return (
    <EngineProvider>
      <WorkspaceBinding />
      <PluginBinding />
      <div class="flex h-full flex-col bg-bg text-ink">
        <Titlebar />
        <div class="flex min-h-0 flex-1">
          <Sidebar />
          <main class="flex min-w-0 flex-1 flex-col">
            <ChatHeader />
            <Chat />
            <AttentionStrip />
            <Composer />
          </main>
        </div>
      </div>
    </EngineProvider>
  )
}

function PluginBinding() {
  const engine = useEngine()
  return <PluginHost engine={engine} />
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
    purge()
  })
  const timer = setInterval(() => purge(), 60 * 60 * 1000)
  onCleanup(() => clearInterval(timer))
  return null

  function purge() {
    if (engine.state.connection !== "online" || Date.now() - lastPurge < dayMs) return
    lastPurge = Date.now()
    void purgeArchived().then((ids) => ids.forEach((id) => void engine.actions.remove(id)))
    void purgeRemovedWorkspaces().then((paths) => paths.forEach((path) => void engine.actions.removeAllSessions(path)))
  }
}
