import { createSignal, For, onCleanup, Show } from "solid-js"
import { useEngine } from "../engine"
import { normalizeDir } from "../engine/store"
import { pickFolder } from "../state/dialog"
import { persisted } from "../state/persist"
import { selectedSession, selectSession } from "../state/selection"
import type { Workspace } from "../state/store"
import { addWorkspace, removedWorkspaces, selectWorkspace, updateWorkspace, workspaces } from "../state/workspaces"
import { ArchiveModal } from "./archive"
import { IconArchive, IconGear, IconPlus } from "./icons"
import { openSettings } from "./settings"
import {
  SessionMenu,
  WorkspaceEditModal,
  WorkspaceGroup,
  WorkspaceMenu,
  type SessionMenuState,
  type WorkspaceMenuState,
} from "./workspaces"

const minSidebarWidth = 192
const maxSidebarWidth = 480
const [storedSidebarWidth, storeSidebarWidth] = persisted("drift.sidebar.width", 256)
const clampSidebarWidth = (width: number) => Math.min(maxSidebarWidth, Math.max(minSidebarWidth, width))

export function sidebarWidthFromDrag(startWidth: number, deltaX: number, scale: number) {
  return clampSidebarWidth(startWidth + deltaX / Math.max(scale, 0.01))
}

export function Sidebar() {
  const engine = useEngine()
  const [menu, setMenu] = createSignal<WorkspaceMenuState | null>(null)
  const [sessionMenu, setSessionMenu] = createSignal<SessionMenuState | null>(null)
  const [editing, setEditing] = createSignal<string | null>(null)
  const [archive, setArchive] = createSignal(false)
  const [moveStatus, setMoveStatus] = createSignal<{ error: boolean; text: string } | null>(null)
  const [width, setWidth] = createSignal(clampSidebarWidth(storedSidebarWidth()))
  let resizeStartX = 0
  let resizeStartWidth = width()
  let resizeScale = 1

  async function add() {
    const path = await pickFolder()
    if (path) await addWorkspace(path)
  }

  async function moveSessionTo(state: SessionMenuState, destination: Workspace) {
    const selected = selectedSession()
    setMoveStatus({ error: false, text: `Moving session to ${destination.name}...` })
    const result = await engine.actions.moveSession(state.sessionId, destination.path)
    if (!result.ok) return setMoveStatus({ error: true, text: result.error ?? "Could not move the session" })
    if (selected && result.moved.includes(selected)) {
      selectWorkspace(destination.id)
      selectSession(selected)
    }
    setMoveStatus(null)
  }

  async function retargetWorkspace(workspace: Workspace) {
    const path = await pickFolder()
    if (!path || normalizeDir(path) === normalizeDir(workspace.path)) return
    const collision = [...workspaces(), ...removedWorkspaces()].find(
      (entry) => entry.id !== workspace.id && normalizeDir(entry.path) === normalizeDir(path),
    )
    if (collision) {
      setMoveStatus({ error: true, text: `${path} is already saved as ${collision.name}` })
      return
    }

    setMoveStatus({ error: false, text: `Moving ${workspace.name} sessions...` })
    const result = await engine.actions.moveWorkspaceSessions(workspace.path, path)
    if (!result.ok) return setMoveStatus({ error: true, text: result.error ?? "Could not move the workspace" })
    try {
      await updateWorkspace(workspace.id, { path })
      setMoveStatus(null)
    } catch (error) {
      const rollback = await engine.actions.moveWorkspaceSessions(path, workspace.path)
      const detail = error instanceof Error ? error.message : String(error)
      setMoveStatus({
        error: true,
        text: rollback.ok ? `Could not save the new workspace path: ${detail}` : `Workspace move was only partially applied: ${detail}`,
      })
    }
  }

  function moveResize(event: PointerEvent) {
    const handle = event.currentTarget as HTMLElement
    if (!handle.hasPointerCapture(event.pointerId)) return
    setWidth(sidebarWidthFromDrag(resizeStartWidth, event.clientX - resizeStartX, resizeScale))
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
        <div class="flex items-center gap-0.5">
          <button
            class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
            title="Archived items"
            onClick={() => setArchive(true)}
          >
            <IconArchive />
          </button>
          <button
            class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
            title="Add workspace (pick a folder)"
            onClick={() => void add()}
          >
            <IconPlus />
          </button>
        </div>
      </div>
      <nav class="min-h-0 flex-1 space-y-2 overflow-y-auto px-2 pb-2">
        <For each={workspaces()}>
          {(workspace) => <WorkspaceGroup workspace={workspace} onMenu={setMenu} onSessionMenu={setSessionMenu} />}
        </For>
        <Show when={workspaces().length === 0}>
          <div class="px-2 py-4 text-xs text-ink-faint">Add a workspace (a project folder) to get started.</div>
        </Show>
      </nav>
      <SidebarFooter onSettings={openSettings} />
      <Show when={menuWorkspace()}>
        {(entry) => (
          <WorkspaceMenu
            state={entry().state}
            workspace={entry().workspace}
            onEdit={() => setEditing(entry().workspace.id)}
            onMove={() => void retargetWorkspace(entry().workspace)}
            onClose={() => setMenu(null)}
          />
        )}
      </Show>
      <Show when={sessionMenu()}>
        {(state) => (
          <SessionMenu
            state={state()}
            workspaces={workspaces()}
            onMove={(workspace) => void moveSessionTo(state(), workspace)}
            onClose={() => setSessionMenu(null)}
          />
        )}
      </Show>
      <Show when={editingWorkspace()}>
        {(workspace) => <WorkspaceEditModal workspace={workspace()} onClose={() => setEditing(null)} />}
      </Show>
      <Show when={archive()}>
        <ArchiveModal onClose={() => setArchive(false)} />
      </Show>
      <Show when={moveStatus()}>
        {(status) => (
          <button
            class="fixed bottom-4 left-1/2 z-50 max-w-lg -translate-x-1/2 rounded-lg border border-edge bg-overlay px-3 py-2 text-xs shadow-xl shadow-black/40"
            classList={{ "text-danger": status().error, "text-ink-muted": !status().error }}
            onClick={() => status().error && setMoveStatus(null)}
          >
            {status().text}
          </button>
        )}
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
          const aside = event.currentTarget.parentElement
          const renderedWidth = aside?.getBoundingClientRect().width ?? width()
          resizeStartX = event.clientX
          resizeStartWidth = width()
          resizeScale = renderedWidth / Math.max(resizeStartWidth, 1)
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
    if (engine.state.startupError) return "engine failed"
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
        <span
          class="flex items-center gap-1.5 text-[0.65rem] text-ink-faint"
          title={engine.state.startupError || engine.state.directory}
        >
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
