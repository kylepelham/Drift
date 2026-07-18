import { createEffect, createMemo, onCleanup, Show, For, type JSX } from "solid-js"
import { useEngine } from "../engine"
import { IconArchive, IconDots, IconSquarePen } from "./icons"
import { sessionBusy, sessionsFor } from "../engine/store"
import { selectedSession, selectSession } from "../state/selection"
import type { Workspace } from "../state/store"
import {
  activeWorkspaceId,
  archivedIds,
  archiveSession,
  removeWorkspace,
  selectWorkspace,
  updateWorkspace,
} from "../state/workspaces"

export type WorkspaceMenuState = { x: number; y: number; workspaceId: string }

export function WorkspaceGroup(props: { workspace: Workspace; onMenu: (state: WorkspaceMenuState) => void }) {
  const engine = useEngine()
  const active = () => activeWorkspaceId() === props.workspace.id
  const sessions = createMemo(() =>
    sessionsFor(engine.state, props.workspace.path).filter((session) => !archivedIds().has(session.id)),
  )
  const openMenu = (x: number, y: number) => props.onMenu({ x, y, workspaceId: props.workspace.id })
  return (
    <div>
      <div
        class="group flex w-full cursor-pointer items-center gap-2.5 rounded-md py-1.5 pr-1.5 pl-2 transition-colors"
        classList={{ "bg-raised": active(), "hover:bg-raised/60": !active() }}
        onClick={() => selectWorkspace(props.workspace.id)}
        onContextMenu={(event) => {
          event.preventDefault()
          openMenu(event.clientX, event.clientY)
        }}
      >
        <WorkspaceIcon workspace={props.workspace} />
        <span class="min-w-0 flex-1 truncate text-sm" classList={{ "text-ink": active(), "text-ink-muted": !active() }}>
          {props.workspace.name}
        </span>
        <div class="items-center" classList={{ flex: active(), "hidden group-hover:flex": !active() }}>
          <RowButton
            title="New thread"
            onClick={() => {
              selectWorkspace(props.workspace.id)
              selectSession(null)
            }}
          >
            <IconSquarePen />
          </RowButton>
          <RowButton
            title="Workspace options"
            onClick={(event) => {
              const rect = (event.currentTarget as HTMLElement).getBoundingClientRect()
              openMenu(rect.left, rect.bottom + 4)
            }}
          >
            <IconDots />
          </RowButton>
        </div>
      </div>
      <div class="mt-0.5 ml-4 space-y-0.5 border-l border-edge pl-1.5">
        <For each={sessions()}>
          {(session) => (
            <ThreadItem
              sessionId={session.id}
              title={session.title}
              updated={session.time.updated}
              workspace={props.workspace}
            />
          )}
        </For>
        <Show when={sessions().length === 0 && active()}>
          <div class="px-2 py-1.5 text-xs text-ink-faint">No threads yet</div>
        </Show>
      </div>
    </div>
  )
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
      <Show when={sessionBusy(engine.state, props.sessionId)}>
        <span class="pulse-soft size-1.5 shrink-0 rounded-full bg-accent" />
      </Show>
      <span class="min-w-0 flex-1 truncate text-[0.8rem]" classList={{ "text-ink": active(), "text-ink-muted": !active() }}>
        {props.title || "Untitled"}
      </span>
      <span class="shrink-0 text-[0.65rem] text-ink-faint group-hover:hidden">{ago(props.updated)}</span>
      <span class="hidden shrink-0 group-hover:block">
        <RowButton
          title="Archive thread"
          onClick={() => {
            if (selectedSession() === props.sessionId) selectSession(null)
            void archiveSession(props.sessionId, props.workspace.id)
          }}
        >
          <IconArchive />
        </RowButton>
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

export function WorkspaceMenu(props: { state: WorkspaceMenuState; workspace: Workspace; onClose: () => void }) {
  let root!: HTMLDivElement

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

  const rename = (name: string) => {
    const next = name.trim()
    if (next && next !== props.workspace.name) void updateWorkspace(props.workspace.id, { name: next })
  }

  async function changeIcon() {
    const icon = await pickIconImage()
    if (icon) await updateWorkspace(props.workspace.id, { icon })
    props.onClose()
  }

  return (
    <div
      ref={root}
      class="fade-up fixed z-40 w-56 rounded-lg border border-edge bg-overlay p-1.5 shadow-xl shadow-black/40"
      style={{ left: `${Math.min(props.state.x, window.innerWidth - 240)}px`, top: `${Math.min(props.state.y, window.innerHeight - 220)}px` }}
    >
      <input
        class="mb-1 w-full rounded-md border border-edge bg-surface px-2 py-1.5 text-sm outline-none focus:border-edge-strong"
        value={props.workspace.name}
        onKeyDown={(event) => {
          if (event.key !== "Enter") return
          rename(event.currentTarget.value)
          props.onClose()
        }}
        onBlur={(event) => rename(event.currentTarget.value)}
      />
      <MenuItem label="Change icon image..." onClick={() => void changeIcon()} />
      <Show when={props.workspace.icon}>
        <MenuItem
          label="Remove icon"
          onClick={() => {
            void updateWorkspace(props.workspace.id, { icon: "" })
            props.onClose()
          }}
        />
      </Show>
      <div class="my-1 h-px bg-edge" />
      <MenuItem
        label="Remove workspace"
        danger
        onClick={() => {
          void removeWorkspace(props.workspace.id)
          props.onClose()
        }}
      />
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
