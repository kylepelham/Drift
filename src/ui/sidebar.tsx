import { createSignal, For, onCleanup, Show } from "solid-js"
import { useEngine } from "../engine"
import { pickFolder } from "../state/dialog"
import { persisted } from "../state/persist"
import { addWorkspace, workspaces } from "../state/workspaces"
import { IconGear, IconPlus } from "./icons"
import { SettingsModal } from "./settings"
import { WorkspaceEditModal, WorkspaceGroup, WorkspaceMenu, type WorkspaceMenuState } from "./workspaces"

const minSidebarWidth = 192
const maxSidebarWidth = 480
const [storedSidebarWidth, storeSidebarWidth] = persisted("drift.sidebar.width", 256)
const clampSidebarWidth = (width: number) => Math.min(maxSidebarWidth, Math.max(minSidebarWidth, width))

export function Sidebar() {
  const [menu, setMenu] = createSignal<WorkspaceMenuState | null>(null)
  const [editing, setEditing] = createSignal<string | null>(null)
  const [settings, setSettings] = createSignal(false)
  const [width, setWidth] = createSignal(clampSidebarWidth(storedSidebarWidth()))

  async function add() {
    const path = await pickFolder()
    if (path) await addWorkspace(path)
  }

  function moveResize(event: PointerEvent) {
    const handle = event.currentTarget as HTMLElement
    if (!handle.hasPointerCapture(event.pointerId)) return
    const left = handle.parentElement?.getBoundingClientRect().left ?? 0
    setWidth(clampSidebarWidth(event.clientX - left))
  }

  function finishResize() {
    document.body.style.cursor = ""
    document.body.style.userSelect = ""
    storeSidebarWidth(width())
  }

  function resizeWithKeyboard(event: KeyboardEvent) {
    const direction = event.key === "ArrowLeft" ? -1 : event.key === "ArrowRight" ? 1 : 0
    if (!direction) return
    event.preventDefault()
    const next = clampSidebarWidth(width() + direction * 16)
    setWidth(next)
    storeSidebarWidth(next)
  }

  onCleanup(finishResize)

  return (
    <aside class="relative flex shrink-0 flex-col border-r border-edge bg-surface" style={{ width: `${width()}px` }}>
      <div class="flex items-center justify-between pt-2.5 pb-1.5 pr-3.5 pl-4">
        <span class="text-[0.68rem] tracking-wider text-ink-faint uppercase">Workspaces</span>
        <button
          class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
          title="Add workspace (pick a folder)"
          onClick={() => void add()}
        >
          <IconPlus />
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
      <div
        role="separator"
        aria-label="Resize sidebar"
        aria-orientation="vertical"
        aria-valuemin={minSidebarWidth}
        aria-valuemax={maxSidebarWidth}
        aria-valuenow={Math.round(width())}
        tabIndex={0}
        class="absolute inset-y-0 right-0 z-20 w-1 translate-x-1/2 cursor-col-resize transition-colors hover:bg-accent/50 focus:bg-accent/50 focus:outline-none"
        onPointerDown={(event) => {
          event.preventDefault()
          event.currentTarget.setPointerCapture(event.pointerId)
          document.body.style.cursor = "col-resize"
          document.body.style.userSelect = "none"
        }}
        onPointerMove={moveResize}
        onPointerUp={(event) => {
          event.currentTarget.releasePointerCapture(event.pointerId)
        }}
        onLostPointerCapture={finishResize}
        onKeyDown={resizeWithKeyboard}
      />
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
