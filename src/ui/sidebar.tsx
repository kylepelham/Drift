import { createSignal, For, Show } from "solid-js"
import { useEngine } from "../engine"
import { pickFolder } from "../state/dialog"
import { cycleTheme, theme } from "../state/theme"
import { addWorkspace, workspaces } from "../state/workspaces"
import { WorkspaceGroup, WorkspaceMenu, type WorkspaceMenuState } from "./workspaces"

export function Sidebar() {
  const [menu, setMenu] = createSignal<WorkspaceMenuState | null>(null)

  async function add() {
    const path = await pickFolder()
    if (path) await addWorkspace(path)
  }

  return (
    <aside class="flex w-64 shrink-0 flex-col border-r border-edge bg-surface">
      <div class="flex items-center justify-between px-4 pt-3 pb-2">
        <span class="text-[0.68rem] tracking-wider text-ink-faint uppercase">Workspaces</span>
        <button
          class="rounded-md border border-edge px-2 py-1 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink"
          title="Add workspace (pick a folder)"
          onClick={() => void add()}
        >
          + Add
        </button>
      </div>
      <nav class="min-h-0 flex-1 space-y-2 overflow-y-auto px-2 pb-2">
        <For each={workspaces()}>
          {(workspace) => <WorkspaceGroup workspace={workspace} onMenu={setMenu} />}
        </For>
        <Show when={workspaces().length === 0}>
          <div class="px-2 py-4 text-xs text-ink-faint">Add a workspace (a project folder) to get started.</div>
        </Show>
      </nav>
      <SidebarFooter />
      <Show when={menuWorkspace()}>
        {(entry) => <WorkspaceMenu state={entry().state} workspace={entry().workspace} onClose={() => setMenu(null)} />}
      </Show>
    </aside>
  )

  function menuWorkspace() {
    const state = menu()
    if (!state) return null
    const workspace = workspaces().find((w) => w.id === state.workspaceId)
    return workspace ? { state, workspace } : null
  }
}

function SidebarFooter() {
  const engine = useEngine()
  const dot: Record<string, string> = {
    online: "bg-ok",
    connecting: "bg-warn pulse-soft",
    offline: "bg-danger",
    idle: "bg-ink-faint",
  }
  const label = () => {
    if (engine.state.connection === "online") return shortPath(engine.state.directory)
    if (engine.state.connection === "idle") return "no workspace"
    return engine.state.connection
  }
  return (
    <div class="flex items-center gap-2 border-t border-edge px-4 py-2.5 text-xs text-ink-faint">
      <span class={`size-1.5 rounded-full ${dot[engine.state.connection]}`} />
      <span class="min-w-0 flex-1 truncate" title={engine.state.directory}>
        {label()}
      </span>
      <button class="transition-colors hover:text-ink" title={`Theme: ${theme()}`} onClick={cycleTheme}>
        <svg class="size-3.5" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
          <circle cx="8" cy="8" r="6" />
          <path d="M8 2a6 6 0 000 12z" fill="currentColor" stroke="none" />
        </svg>
      </button>
    </div>
  )
}

function shortPath(path: string) {
  const parts = path.replaceAll("\\", "/").split("/").filter(Boolean)
  return parts.slice(-2).join("/") || path
}
