import { createEffect, createMemo, createSignal, Match, onCleanup, Show, Switch, For, type JSX } from "solid-js"
import { useEngine } from "../engine"
import { emitThreadArchived } from "../plugins"
import { IconArchive, IconBranch, IconDots, IconSquarePen } from "./icons"
import { childrenOf, sessionBusy, sessionsFor } from "../engine/store"
import { selectedSession, selectSession } from "../state/selection"
import type { Workspace } from "../state/store"
import { Chevron } from "./parts"
import {
  activeWorkspaceId,
  archivedIds,
  archiveSession,
  moveWorkspace,
  removeWorkspace,
  selectWorkspace,
  toggleWorkspaceCollapsed,
  updateWorkspace,
  workspaceCollapsed,
} from "../state/workspaces"

export type WorkspaceMenuState = { x: number; y: number; workspaceId: string }

type SessionList = ReturnType<typeof sessionsFor>
const sessionPageSize = 5

// ponytail: last-known lists mask the engine store reset while switching workspaces
const sessionListCache = new Map<string, SessionList>()

export function WorkspaceGroup(props: { workspace: Workspace; onMenu: (state: WorkspaceMenuState) => void }) {
  const engine = useEngine()
  let root!: HTMLDivElement
  const collapsed = () => workspaceCollapsed(props.workspace.id)
  const [visibleCount, setVisibleCount] = createSignal(sessionPageSize)
  const active = () => activeWorkspaceId() === props.workspace.id
  const all = createMemo(() => {
    const live = sessionsFor(engine.state, props.workspace.path)
    if (live.length) sessionListCache.set(props.workspace.path, live)
    return live.length ? live : (sessionListCache.get(props.workspace.path) ?? live)
  })
  const children = (parentId: string) =>
    childrenOf(engine.state, parentId).filter((child) => sessionBusy(engine.state, child.id))
  const sessions = createMemo(() => all().filter((session) => !archivedIds().has(session.id)))
  const visibleSessions = createMemo(() => sessions().slice(0, visibleCount()))
  const remaining = createMemo(() => Math.max(0, sessions().length - visibleSessions().length))
  const openMenu = (x: number, y: number) => props.onMenu({ x, y, workspaceId: props.workspace.id })
  return (
    <div ref={root} data-workspace={props.workspace.id}>
      <div
        class="group flex w-full cursor-pointer items-center gap-2.5 rounded-md py-1.5 pr-1.5 pl-2 transition-colors"
        classList={{ "bg-raised": active(), "hover:bg-raised/60": !active() }}
        onPointerDown={(event) => dragWorkspace(event, root, props.workspace.id)}
        onClick={() => {
          if (dragged) return
          selectWorkspace(props.workspace.id)
          toggleWorkspaceCollapsed(props.workspace.id)
        }}
        onContextMenu={(event) => {
          event.preventDefault()
          openMenu(event.clientX, event.clientY)
        }}
      >
        <button
          title={collapsed() ? "Show threads" : "Hide threads"}
          aria-expanded={!collapsed()}
          class="-mr-1 -ml-1 flex size-5 shrink-0 items-center justify-center rounded text-ink-faint transition-colors hover:bg-overlay hover:text-ink"
          onPointerDown={(event) => event.stopPropagation()}
          onClick={(event) => {
            event.stopPropagation()
            toggleWorkspaceCollapsed(props.workspace.id)
          }}
        >
          <Chevron open={!collapsed()} />
        </button>
        <WorkspaceIcon workspace={props.workspace} />
        <span class="min-w-0 flex-1 truncate text-sm" classList={{ "text-ink": active(), "text-ink-muted": !active() }}>
          {props.workspace.name}
        </span>
        <div class="flex items-center" classList={{ "invisible group-hover:visible": !active() }}>
          <RowButton
            title="Workspace options"
            onClick={(event) => {
              const rect = (event.currentTarget as HTMLElement).getBoundingClientRect()
              openMenu(rect.left, rect.bottom + 4)
            }}
          >
            <IconDots />
          </RowButton>
          <RowButton
            title="New thread"
            onClick={() => {
              selectWorkspace(props.workspace.id)
              selectSession(null)
            }}
          >
            <IconSquarePen />
          </RowButton>
        </div>
      </div>
      <Show when={!collapsed()}>
        <div class="mt-0.5 ml-4 space-y-0.5 border-l border-edge pl-1.5">
          <For each={visibleSessions()}>
            {(session) => (
              <>
                <ThreadItem
                  sessionId={session.id}
                  title={session.title}
                  updated={session.time.updated}
                  workspace={props.workspace}
                />
                <For each={children(session.id)}>
                  {(child) => <ChildThreadItem sessionId={child.id} title={child.title} workspace={props.workspace} />}
                </For>
              </>
            )}
          </For>
          <Show when={remaining() > 0}>
            <button
              class="flex h-7 w-full items-center rounded-md px-2 text-left text-[0.72rem] text-ink-faint transition-colors hover:bg-raised/60 hover:text-ink-muted"
              onClick={() => setVisibleCount((count) => Math.min(count + sessionPageSize, sessions().length))}
            >
              Load {Math.min(sessionPageSize, remaining())} more
            </button>
          </Show>
          <Show when={sessions().length === 0 && active()}>
            <div class="px-2 py-1.5 text-xs text-ink-faint">No threads yet</div>
          </Show>
        </div>
      </Show>
    </div>
  )
}

let dragged = false

type DragBox = { id: string; el: HTMLElement; mid: number }

function dragWorkspace(event: PointerEvent, root: HTMLElement, id: string) {
  if (event.button !== 0) return
  const header = event.currentTarget as HTMLElement
  const startY = event.clientY
  let boxes: DragBox[] = []
  let origIndex = 0
  let slot = 0
  let target = 0
  let active = false
  let minDy = 0
  let maxDy = 0
  let rectTop = 0
  let rectBottom = 0

  const begin = () => {
    active = true
    header.setPointerCapture(event.pointerId)
    const els = Array.from(root.parentElement?.querySelectorAll<HTMLElement>(":scope > [data-workspace]") ?? [])
    boxes = els.map((el) => {
      const rect = el.getBoundingClientRect()
      return { id: el.dataset.workspace ?? "", el, mid: rect.top + rect.height / 2 }
    })
    origIndex = boxes.findIndex((box) => box.el === root)
    const rect = root.getBoundingClientRect()
    slot = rect.height + 8
    rectTop = rect.top
    rectBottom = rect.bottom
    minDy = els[0].getBoundingClientRect().top - rect.top
    maxDy = els[els.length - 1].getBoundingClientRect().bottom - rect.bottom
    root.style.position = "relative"
    root.style.zIndex = "10"
    for (const box of boxes) if (box.el !== root) box.el.style.transition = "transform 150ms ease"
  }

  const onMove = (e: PointerEvent) => {
    let dy = e.clientY - startY
    if (!active && Math.abs(dy) < 5) return
    if (!active) begin()
    dy = Math.min(maxDy, Math.max(minDy, dy))
    root.style.transform = `translateY(${dy}px)`
    const others = boxes.filter((box) => box.el !== root)
    target = others.filter((box, i) => box.mid < (i < origIndex ? rectTop : rectBottom) + dy).length
    others.forEach((box, i) => {
      const shift = i >= target && i < origIndex ? slot : i >= origIndex && i < target ? -slot : 0
      box.el.style.transform = shift ? `translateY(${shift}px)` : ""
    })
  }

  const onUp = () => {
    header.removeEventListener("pointermove", onMove)
    header.removeEventListener("pointerup", onUp)
    if (!active) return
    for (const box of boxes) {
      box.el.style.transform = ""
      box.el.style.transition = ""
    }
    root.style.position = ""
    root.style.zIndex = ""
    const others = boxes.filter((box) => box.el !== root).map((box) => box.id)
    moveWorkspace(id, others[target] ?? null)
    dragged = true
    setTimeout(() => (dragged = false), 0)
  }

  header.addEventListener("pointermove", onMove)
  header.addEventListener("pointerup", onUp)
}

function RowButton(props: { title: string; onClick: (event: MouseEvent) => void; children: JSX.Element }) {
  return (
    <button
      title={props.title}
      class="flex size-7 shrink-0 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-overlay hover:text-ink"
      onClick={(event) => {
        event.stopPropagation()
        props.onClick(event)
      }}
    >
      {props.children}
    </button>
  )
}

function ThreadItem(props: { sessionId: string; title: string; updated: number; workspace: Workspace }) {
  const engine = useEngine()
  const active = () => selectedSession() === props.sessionId
  return (
    <div
      class="group flex h-8 cursor-pointer items-center gap-2 rounded-md py-1 pr-1 pl-2 transition-colors"
      classList={{ "bg-raised": active(), "hover:bg-raised/60": !active() }}
      onClick={() => {
        selectWorkspace(props.workspace.id)
        selectSession(props.sessionId)
      }}
    >
      <StatusDot sessionId={props.sessionId} />
      <span class="min-w-0 flex-1 truncate text-[0.8rem]" classList={{ "text-ink": active(), "text-ink-muted": !active() }}>
        {props.title || "Untitled"}
      </span>
      <span class="shrink-0 text-[0.65rem] text-ink-faint group-hover:hidden">{ago(props.updated)}</span>
      <span class="hidden shrink-0 items-center group-hover:flex">
        <RowButton
          title="Fork thread (duplicate with full history)"
          onClick={() => {
            void engine.actions.fork(props.sessionId).then((session) => session && selectSession(session.id))
          }}
        >
          <IconBranch />
        </RowButton>
        <RowButton
          title="Archive thread"
          onClick={() => {
            if (selectedSession() === props.sessionId) selectSession(null)
            void archiveSession(props.sessionId, props.workspace.id)
            emitThreadArchived(props.sessionId)
          }}
        >
          <IconArchive />
        </RowButton>
      </span>
    </div>
  )
}

function StatusDot(props: { sessionId: string }) {
  const engine = useEngine()
  const attention = () =>
    (engine.state.permissions[props.sessionId]?.length ?? 0) > 0 ||
    (engine.state.questions[props.sessionId]?.length ?? 0) > 0
  return (
    <Switch>
      <Match when={attention()}>
        <span class="size-1.5 shrink-0 rounded-full bg-warn" title="Waiting for permission" />
      </Match>
      <Match when={sessionBusy(engine.state, props.sessionId)}>
        <span class="pulse-soft size-1.5 shrink-0 rounded-full bg-accent" />
      </Match>
    </Switch>
  )
}

function ChildThreadItem(props: { sessionId: string; title: string; workspace: Workspace }) {
  const active = () => selectedSession() === props.sessionId
  return (
    <div
      class="flex h-7 cursor-pointer items-center gap-1.5 rounded-md py-0.5 pr-2 pl-5 transition-colors"
      classList={{ "bg-raised": active(), "hover:bg-raised/60": !active() }}
      onClick={() => {
        selectWorkspace(props.workspace.id)
        selectSession(props.sessionId)
      }}
    >
      <span class="text-[0.7rem] text-ink-faint">&#8627;</span>
      <StatusDot sessionId={props.sessionId} />
      <span class="min-w-0 flex-1 truncate text-[0.75rem]" classList={{ "text-ink": active(), "text-ink-faint": !active() }}>
        {props.title || "Spawned thread"}
      </span>
    </div>
  )
}

const hues = [212, 262, 330, 24, 96, 168]

export function WorkspaceIcon(props: { workspace: Workspace }) {
  const hue = () => {
    let hash = 0
    for (const char of props.workspace.path) hash = (hash * 31 + char.charCodeAt(0)) | 0
    return hues[Math.abs(hash) % hues.length]
  }
  return (
    <Show
      when={props.workspace.icon.startsWith("data:")}
      fallback={
        <span
          class="flex size-6 shrink-0 items-center justify-center rounded-md text-[0.65rem] font-semibold text-white/90"
          style={{ background: `hsl(${hue()} 40% 34%)` }}
        >
          {initials(props.workspace.name)}
        </span>
      }
    >
      <img src={props.workspace.icon} alt="" class="size-6 shrink-0 rounded-md object-cover" />
    </Show>
  )
}

function initials(name: string) {
  const words = name.split(/[\s\-_.]+/).filter(Boolean)
  const letters = words.slice(0, 2).map((word) => word.charAt(0))
  return (letters.join("") || name.charAt(0)).toUpperCase()
}

export function WorkspaceMenu(props: {
  state: WorkspaceMenuState
  workspace: Workspace
  onEdit: () => void
  onClose: () => void
}) {
  let root!: HTMLDivElement
  const [confirming, setConfirming] = createSignal(false)

  createEffect(() => {
    const away = (event: MouseEvent) => {
      if (!root.contains(event.target as Node)) props.onClose()
    }
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") props.onClose()
    }
    document.addEventListener("mousedown", away)
    document.addEventListener("keydown", escape)
    onCleanup(() => {
      document.removeEventListener("mousedown", away)
      document.removeEventListener("keydown", escape)
    })
  })

  return (
    <div
      ref={root}
      class="fade-up fixed z-40 w-52 rounded-lg border border-edge bg-overlay p-1.5 shadow-xl shadow-black/40"
      style={{
        left: `${Math.min(props.state.x, window.innerWidth - 220)}px`,
        top: `${Math.min(props.state.y, window.innerHeight - 120)}px`,
      }}
    >
      <MenuItem
        label="Edit"
        onClick={() => {
          props.onEdit()
          props.onClose()
        }}
      />
      <MenuItem
        label={confirming() ? "Click again to confirm" : "Remove"}
        danger
        onClick={() => {
          if (!confirming()) return setConfirming(true)
          void removeWorkspace(props.workspace.id)
          props.onClose()
        }}
      />
      <Show when={confirming()}>
        <div class="px-2 pt-1 pb-0.5 text-[0.65rem] leading-snug text-ink-faint">
          Threads are kept for 7 days; re-add the same folder to restore them.
        </div>
      </Show>
    </div>
  )
}

export function WorkspaceEditModal(props: { workspace: Workspace; onClose: () => void }) {
  const [name, setName] = createSignal(props.workspace.name)
  const [icon, setIcon] = createSignal(props.workspace.icon)

  createEffect(() => {
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") props.onClose()
    }
    document.addEventListener("keydown", escape)
    onCleanup(() => document.removeEventListener("keydown", escape))
  })

  async function save() {
    const next = name().trim()
    await updateWorkspace(props.workspace.id, { name: next || props.workspace.name, icon: icon() })
    props.onClose()
  }

  return (
    <div class="fixed inset-0 z-30 flex items-center justify-center bg-black/50" onClick={props.onClose}>
      <div
        class="fade-up w-96 rounded-xl border border-edge bg-overlay p-4 shadow-2xl shadow-black/40"
        onClick={(event) => event.stopPropagation()}
      >
        <div class="mb-4 text-sm font-semibold text-ink">Edit workspace</div>
        <div class="mb-4 flex items-center gap-3">
          <Show
            when={icon().startsWith("data:")}
            fallback={
              <span class="flex size-12 items-center justify-center rounded-lg bg-raised text-sm font-semibold text-ink-muted">
                {initials(name() || props.workspace.name)}
              </span>
            }
          >
            <img src={icon()} alt="" class="size-12 rounded-lg object-cover" />
          </Show>
          <div class="flex flex-col gap-1.5">
            <button
              class="rounded-md border border-edge px-2.5 py-1 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink"
              onClick={() => void pickIconImage().then((image) => image && setIcon(image))}
            >
              Change image...
            </button>
            <Show when={icon()}>
              <button
                class="rounded-md border border-edge px-2.5 py-1 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink"
                onClick={() => setIcon("")}
              >
                Use initials
              </button>
            </Show>
          </div>
        </div>
        <label class="mb-4 block">
          <span class="mb-1 block text-[0.68rem] tracking-wide text-ink-faint uppercase">Name</span>
          <input
            class="w-full rounded-md border border-edge bg-surface px-2.5 py-1.5 text-sm outline-none focus:border-edge-strong"
            value={name()}
            onInput={(event) => setName(event.currentTarget.value)}
            onKeyDown={(event) => event.key === "Enter" && void save()}
          />
        </label>
        <div class="mb-3 text-[0.68rem] text-ink-faint">{props.workspace.path}</div>
        <div class="flex justify-end gap-2">
          <button
            class="rounded-md border border-edge px-3 py-1.5 text-xs text-ink-muted transition-colors hover:text-ink"
            onClick={props.onClose}
          >
            Cancel
          </button>
          <button
            class="rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-accent-ink"
            onClick={() => void save()}
          >
            Save
          </button>
        </div>
      </div>
    </div>
  )
}

function MenuItem(props: { label: string; danger?: boolean; onClick: () => void }) {
  return (
    <button
      class="w-full rounded-md px-2 py-1.5 text-left text-sm transition-colors"
      classList={{
        "text-ink-muted hover:bg-raised hover:text-ink": !props.danger,
        "text-danger hover:bg-danger/10": props.danger,
      }}
      onClick={props.onClick}
    >
      {props.label}
    </button>
  )
}

async function pickIconImage(): Promise<string | null> {
  const file = await pickFile("image/*")
  if (!file) return null
  const bitmap = await createImageBitmap(file)
  const size = 64
  const canvas = document.createElement("canvas")
  canvas.width = size
  canvas.height = size
  const context = canvas.getContext("2d")!
  const scale = Math.max(size / bitmap.width, size / bitmap.height)
  const width = bitmap.width * scale
  const height = bitmap.height * scale
  context.drawImage(bitmap, (size - width) / 2, (size - height) / 2, width, height)
  bitmap.close()
  return canvas.toDataURL("image/webp", 0.85)
}

function pickFile(accept: string): Promise<File | null> {
  return new Promise((resolve) => {
    const input = document.createElement("input")
    input.type = "file"
    input.accept = accept
    input.onchange = () => resolve(input.files?.[0] ?? null)
    input.oncancel = () => resolve(null)
    input.click()
  })
}

export function ago(timestamp: number) {
  const seconds = Math.max(0, (Date.now() - timestamp) / 1000)
  if (seconds < 60) return "just now"
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`
  return `${Math.floor(seconds / 86400)}d ago`
}
