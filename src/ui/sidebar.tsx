import { createSignal, For, Show } from "solid-js"
import { useEngine } from "../engine"
import { pickFolder } from "../state/dialog"
import { addWorkspace, workspaces } from "../state/workspaces"
import { IconGear } from "./icons"
import { SettingsModal } from "./settings"
import { WorkspaceEditModal, WorkspaceGroup, WorkspaceMenu, type WorkspaceMenuState } from "./workspaces"

export function Sidebar() {
  const [menu, setMenu] = createSignal<WorkspaceMenuState | null>(null)
  const [editing, setEditing] = createSignal<string | null>(null)
  const [settings, setSettings] = createSignal(false)

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
        <For each={workspaces()}>{(workspace) => <WorkspaceGroup workspace={workspace} onMenu={setMenu} />}</For>
        <Show when={workspaces().length === 0}>
          <div class="px-2 py-4 text-xs text-ink-faint">Add a workspace (a project folder) to get started.</div>
        </Show>
      </nav>
      <SidebarFooter onSettings={() => setSettings(true)} />
      <Show when={menuWorkspace()}>
        {(entry) => (
          <WorkspaceMenu
            state={entry().state}
            workspace={entry().workspace}
            onEdit={() => setEditing(entry().workspace.id)}
            onClose={() => setMenu(null)}
          />
        )}
      </Show>
      <Show when={editingWorkspace()}>
        {(workspace) => <WorkspaceEditModal workspace={workspace()} onClose={() => setEditing(null)} />}
      </Show>
      <Show when={settings()}>
        <SettingsModal onClose={() => setSettings(false)} />
      </Show>
    </aside>
  )

  function menuWorkspace() {
    const state = menu()
    if (!state) return null
    const workspace = workspaces().find((w) => w.id === state.workspaceId)
    return workspace ? { state, workspace } : null
  }

  function editingWorkspace() {
    return workspaces().find((w) => w.id === editing()) ?? null
  }
}

function SidebarFooter(props: { onSettings: () => void }) {
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
    <div class="border-t border-edge px-2 py-2">
      <button
        class="flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-sm text-ink-muted transition-colors hover:bg-raised hover:text-ink"
        onClick={props.onSettings}
      >
        <IconGear />
        <span>Settings</span>
        <span class="flex-1" />
        <span class="flex items-center gap-1.5 text-[0.65rem] text-ink-faint" title={engine.state.directory}>
          <span class={`size-1.5 rounded-full ${dot[engine.state.connection]}`} />
          <span class="max-w-24 truncate">{label()}</span>
        </span>
      </button>
    </div>
  )
}

function shortPath(path: string) {
  const parts = path.replaceAll("\\", "/").split("/").filter(Boolean)
  return parts.slice(-2).join("/") || path
}
