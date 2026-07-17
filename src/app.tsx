import { createEffect, onMount } from "solid-js"
import { EngineProvider, useEngine } from "./engine"
import { bindTheme } from "./state/theme"
import { activeWorkspace, initWorkspaces, purgeArchived, workspaces } from "./state/workspaces"
import { AttentionStrip } from "./ui/attention"
import { Chat } from "./ui/chat"
import { Composer } from "./ui/composer"
import { Sidebar } from "./ui/sidebar"
import { Titlebar } from "./ui/titlebar"

export function App() {
  bindTheme()
  return (
    <EngineProvider>
      <WorkspaceBinding />
      <div class="flex h-full flex-col bg-bg text-ink">
        <Titlebar />
        <div class="flex min-h-0 flex-1">
          <Sidebar />
          <main class="flex min-w-0 flex-1 flex-col">
            <Chat />
            <AttentionStrip />
            <Composer />
          </main>
        </div>
      </div>
    </EngineProvider>
  )
}

function WorkspaceBinding() {
  const engine = useEngine()
  let purged = false
  onMount(() => void initWorkspaces())
  createEffect(() => engine.setDirectory(activeWorkspace()?.path ?? null))
  createEffect(() => {
    if (engine.state.connection !== "online") return
    for (const workspace of workspaces()) void engine.actions.loadSessions(workspace.path)
    if (purged) return
    purged = true
    void purgeArchived().then((ids) => ids.forEach((id) => void engine.actions.remove(id)))
  })
  return null
}

