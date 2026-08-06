import { createMemo, createSignal, Match, onCleanup, onMount, Show, Switch, For, type JSX } from "solid-js"
import { useEngine } from "../engine"
import { createDismissOnOutside } from "./dismiss"
import { emitThreadArchived } from "../plugins"
import { IconArchive, IconBranch, IconDots, IconSquarePen } from "./icons"
import { childrenOf, normalizeDir, sessionBusy, sessionsFor } from "../engine/store"
import { selectedSession, selectSession } from "../state/selection"
import type { Workspace } from "../state/store"
import { fixedMenuPosition } from "../state/zoom"
import { t } from "../state/i18n"
import { Chevron } from "./controls"
import { permissionRequiresAttention } from "../state/permission-attention"
import { dragReorder } from "./drag-reorder"
import { recoverableForSession, recoverableInterruptions } from "../state/recovery"
import { activateModal, closeOnBackdropPointerDown } from "./modal"
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
export type SessionMenuState = { x: number; y: number; sessionId: string; workspaceId: string }

type SessionList = ReturnType<typeof sessionsFor>
const sessionPageSize = 5

// ponytail: last-known lists mask the engine store reset while switching workspaces
const sessionListCache = new Map<string, SessionList>()

export function WorkspaceGroup(props: {
  workspace: Workspace
  onMenu: (state: WorkspaceMenuState) => void
  onSessionMenu: (state: SessionMenuState) => void
}) {
  const engine = useEngine()
  let root!: HTMLDivElement
  let cancelDrag = () => {}
  onCleanup(() => cancelDrag())
  const collapsed = () => workspaceCollapsed(props.workspace.id)
  const [visibleCount, setVisibleCount] = createSignal(sessionPageSize)
  const active = () => activeWorkspaceId() === props.workspace.id
  const all = createMemo(() => {
    const live = sessionsFor(engine.state, props.workspace.path)
    if (live.length) sessionListCache.set(props.workspace.path, live)
    if (engine.state.connection === "online" && !live.length) sessionListCache.delete(props.workspace.path)
    return live.length || engine.state.connection === "online" ? live : (sessionListCache.get(props.workspace.path) ?? live)
  })
  const children = (parentId: string) => {
    recoverableInterruptions()
    return childrenOf(engine.state, parentId).filter(
      (child) => sessionBusy(engine.state, child.id) || !!recoverableForSession(child.id),
    )
  }
  const sessions = createMemo(() => all().filter((session) => !archivedIds().has(session.id)))
  const visibleSessions = createMemo(() => sessions().slice(0, visibleCount()))
  const remaining = createMemo(() => Math.max(0, sessions().length - visibleSessions().length))
  const openMenu = (x: number, y: number) => props.onMenu({ x, y, workspaceId: props.workspace.id })
  return (
    <div ref={root} data-workspace={props.workspace.id}>
      <div
        class="group sticky top-0 z-[1] flex w-full cursor-pointer items-center gap-2.5 rounded-md py-1.5 pr-1.5 pl-2 transition-colors"
        classList={{ "bg-raised": active(), "bg-surface hover:bg-raised/60": !active() }}
        onPointerDown={(event) => {
          cancelDrag()
          cancelDrag = dragReorder(event, root, {
            selector: ":scope > [data-workspace]",
            id: props.workspace.id,
            itemID: (element) => element.dataset.workspace ?? "",
            move: moveWorkspace,
            dragged: markWorkspaceDragged,
          })
        }}
        onClick={() => {
          if (dragged) return
          toggleWorkspaceCollapsed(props.workspace.id)
        }}
        onContextMenu={(event) => {
          event.preventDefault()
          openMenu(event.clientX, event.clientY)
        }}
      >
        <button
          title={collapsed() ? t("drift.workspace.showThreads") : t("drift.workspace.hideThreads")}
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
            title={t("common.moreOptions")}
            onClick={(event) => {
              const rect = (event.currentTarget as HTMLElement).getBoundingClientRect()
              openMenu(rect.left, rect.bottom + 4)
            }}
          >
            <IconDots />
          </RowButton>
          <RowButton
            title={t("drift.thread.new")}
            navigation
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
                  onMenu={props.onSessionMenu}
                />
                <For each={children(session.id)}>
                  {(child) => (
                    <ChildThreadItem
                      sessionId={child.id}
                      title={child.title}
                      workspace={props.workspace}
                      onMenu={props.onSessionMenu}
                    />
                  )}
                </For>
              </>
            )}
          </For>
          <Show when={remaining() > 0}>
            <button
              class="flex h-7 w-full items-center rounded-md px-2 text-left text-[0.72rem] text-ink-faint transition-colors hover:bg-raised/60 hover:text-ink-muted"
              onClick={() => setVisibleCount((count) => Math.min(count + sessionPageSize, sessions().length))}
            >
              {t("drift.thread.loadMore", { count: Math.min(sessionPageSize, remaining()) })}
            </button>
          </Show>
          <Show when={sessions().length === 0 && active()}>
            <div class="px-2 py-1.5 text-xs text-ink-faint">{t("drift.thread.empty")}</div>
          </Show>
        </div>
      </Show>
    </div>
  )
}

let dragged = false

function markWorkspaceDragged() {
  dragged = true
  setTimeout(() => (dragged = false), 0)
}

function RowButton(props: { title: string; navigation?: boolean; onClick: (event: MouseEvent) => void; children: JSX.Element }) {
  return (
    <button
      title={props.title}
      data-sidebar-navigation={props.navigation ? "" : undefined}
      class="flex size-7 shrink-0 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-overlay hover:text-ink"
      onPointerDown={(event) => event.stopPropagation()}
      onClick={(event) => {
        event.stopPropagation()
        props.onClick(event)
      }}
    >
      {props.children}
    </button>
  )
}

function ThreadItem(props: {
  sessionId: string
  title: string
  updated: number
  workspace: Workspace
  onMenu: (state: SessionMenuState) => void
}) {
  const engine = useEngine()
  const active = () => selectedSession() === props.sessionId
  return (
    <div
      data-sidebar-navigation
      class="group flex h-8 cursor-pointer items-center gap-2 rounded-md py-1 pr-1 pl-2 transition-colors"
      classList={{ "bg-raised": active(), "hover:bg-raised/60": !active() }}
      onClick={() => {
        selectWorkspace(props.workspace.id)
        selectSession(props.sessionId)
      }}
      onContextMenu={(event) => {
        event.preventDefault()
        props.onMenu({ x: event.clientX, y: event.clientY, sessionId: props.sessionId, workspaceId: props.workspace.id })
      }}
    >
      <StatusDot sessionId={props.sessionId} />
      <span class="min-w-0 flex-1 truncate text-[0.8rem]" classList={{ "text-ink": active(), "text-ink-muted": !active() }}>
        {props.title || t("drift.thread.untitled")}
      </span>
      <span class="shrink-0 text-[0.65rem] text-ink-faint group-hover:hidden">{ago(props.updated)}</span>
      <span class="hidden shrink-0 items-center group-hover:flex">
        <RowButton
          title={t("drift.slash.fork.all.description")}
          onClick={() => {
            selectWorkspace(props.workspace.id)
            const selection = selectedSession()
            void engine.actions
              .fork(props.sessionId, "full")
              .then(
                (session) =>
                  session &&
                  activeWorkspaceId() === props.workspace.id &&
                  selectedSession() === selection &&
                  selectSession(session.id),
              )
          }}
        >
          <IconBranch />
        </RowButton>
        <RowButton
          title={t("command.session.archive")}
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
  const permissions = () =>
    (engine.state.permissions[props.sessionId] ?? []).filter((permission) =>
      permissionRequiresAttention(permission, engine.state),
    )
  const attention = () =>
    permissions().length > 0 || (engine.state.questions[props.sessionId]?.length ?? 0) > 0
  const attentionTitle = () =>
    permissions().length > 0
      ? t("drift.thread.waitingForPermission")
      : t("drift.thread.waitingForAnswer")
  return (
    <Switch>
      <Match when={(recoverableInterruptions(), recoverableForSession(props.sessionId))}>
        <span class="size-1.5 shrink-0 rounded-full bg-warn" title={t("drift.recovery.title")} />
      </Match>
      <Match when={attention()}>
        <span class="size-1.5 shrink-0 rounded-full bg-warn" title={attentionTitle()} />
      </Match>
      <Match when={sessionBusy(engine.state, props.sessionId)}>
        <span class="pulse-soft size-1.5 shrink-0 rounded-full bg-accent" title={t("drift.thread.working")} />
      </Match>
      <Match when={engine.state.errors[props.sessionId]}>
        <span class="size-1.5 shrink-0 rounded-full bg-danger" title={t("notification.session.error.title")} />
      </Match>
    </Switch>
  )
}

function ChildThreadItem(props: {
  sessionId: string
  title: string
  workspace: Workspace
  onMenu: (state: SessionMenuState) => void
}) {
  const active = () => selectedSession() === props.sessionId
  return (
    <div
      data-sidebar-navigation
      class="flex h-7 cursor-pointer items-center gap-1.5 rounded-md py-0.5 pr-2 pl-5 transition-colors"
      classList={{ "bg-raised": active(), "hover:bg-raised/60": !active() }}
      onClick={() => {
        selectWorkspace(props.workspace.id)
        selectSession(props.sessionId)
      }}
      onContextMenu={(event) => {
        event.preventDefault()
        props.onMenu({ x: event.clientX, y: event.clientY, sessionId: props.sessionId, workspaceId: props.workspace.id })
      }}
    >
      <span class="text-[0.7rem] text-ink-faint">&#8627;</span>
      <StatusDot sessionId={props.sessionId} />
      <span class="min-w-0 flex-1 truncate text-[0.75rem]" classList={{ "text-ink": active(), "text-ink-faint": !active() }}>
        {props.title || t("drift.thread.untitled")}
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
  onMove: () => void
  onClose: () => void
}) {
  let root!: HTMLDivElement
  const [confirming, setConfirming] = createSignal(false)
  const position = () => fixedMenuPosition(props.state.x, props.state.y, 212, confirming() ? 176 : 144)

  createDismissOnOutside({ inside: () => [root], onDismiss: () => props.onClose(), escape: true })

  return (
    <div
      ref={root}
      class="fade-up fixed z-40 w-52 rounded-lg border border-edge bg-overlay p-1.5 shadow-xl shadow-black/40"
      style={{
        left: `${position().left}px`,
        top: `${position().top}px`,
      }}
    >
      <MenuItem
        label={t("common.edit")}
        onClick={() => {
          props.onEdit()
          props.onClose()
        }}
      />
      <MenuItem
        label={t("drift.workspace.move")}
        onClick={() => {
          props.onMove()
          props.onClose()
        }}
      />
      <MenuItem
        label={confirming() ? t("drift.workspace.confirmRemove") : t("drift.workspace.remove")}
        danger
        onClick={() => {
          if (!confirming()) return setConfirming(true)
          void removeWorkspace(props.workspace.id)
          props.onClose()
        }}
      />
      <Show when={confirming()}>
        <div class="px-2 pt-1 pb-0.5 text-[0.65rem] leading-snug text-ink-faint">{t("drift.workspace.removeHint")}</div>
      </Show>
    </div>
  )
}

export function SessionMenu(props: {
  state: SessionMenuState
  workspaces: Workspace[]
  onMove: (workspace: Workspace) => void
  onClose: () => void
}) {
  let root!: HTMLDivElement
  const [choosing, setChoosing] = createSignal(false)
  const targets = () => {
    const source = props.workspaces.find((workspace) => workspace.id === props.state.workspaceId)
    return props.workspaces.filter(
      (workspace) =>
        workspace.id !== props.state.workspaceId && (!source || normalizeDir(workspace.path) !== normalizeDir(source.path)),
    )
  }
  const height = () => (choosing() ? Math.min(320, 48 + Math.max(1, targets().length) * 36) : 48)
  const position = () => fixedMenuPosition(props.state.x, props.state.y, 212, height())

  createDismissOnOutside({ inside: () => [root], onDismiss: () => props.onClose(), escape: true })

  return (
    <div
      ref={root}
      class="fade-up fixed z-40 max-h-80 w-52 overflow-y-auto rounded-lg border border-edge bg-overlay p-1.5 shadow-xl shadow-black/40"
      style={{ left: `${position().left}px`, top: `${position().top}px` }}
    >
      <MenuItem
        label={choosing() ? t("drift.thread.moveToWorkspace") : t("drift.thread.move")}
        onClick={() => setChoosing(true)}
      />
      <Show when={choosing()}>
        <div class="my-1 border-t border-edge" />
        <For each={targets()}>
          {(workspace) => (
            <MenuItem
              label={workspace.name}
              onClick={() => {
                props.onMove(workspace)
                props.onClose()
              }}
            />
          )}
        </For>
        <Show when={targets().length === 0}>
          <MenuItem label={t("drift.thread.noOtherWorkspaces")} disabled onClick={() => {}} />
        </Show>
      </Show>
    </div>
  )
}

export function WorkspaceEditModal(props: { workspace: Workspace; onClose: () => void }) {
  let dialog!: HTMLDivElement
  const [name, setName] = createSignal(props.workspace.name)
  const [icon, setIcon] = createSignal(props.workspace.icon)
  onMount(() => onCleanup(activateModal(dialog, props.onClose)))

  async function save() {
    const next = name().trim()
    await updateWorkspace(props.workspace.id, { name: next || props.workspace.name, icon: icon() })
    props.onClose()
  }

  return (
    <div
      data-modal-layer
      class="fixed inset-0 z-30 flex items-center justify-center bg-black/50"
      onPointerDown={(event) => closeOnBackdropPointerDown(event, props.onClose, dialog)}
    >
      <div
        ref={dialog}
        role="dialog"
        aria-modal="true"
        aria-label={t("dialog.project.edit.title")}
        tabIndex={-1}
        class="fade-up w-96 rounded-xl border border-edge bg-overlay p-4 shadow-2xl shadow-black/40"
        onClick={(event) => event.stopPropagation()}
      >
        <div class="mb-4 text-sm font-semibold text-ink">{t("dialog.project.edit.title")}</div>
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
              {t("drift.workspace.changeImage")}
            </button>
            <Show when={icon()}>
              <button
                class="rounded-md border border-edge px-2.5 py-1 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink"
                onClick={() => setIcon("")}
              >
                {t("drift.workspace.useInitials")}
              </button>
            </Show>
          </div>
        </div>
        <label class="mb-4 block">
          <span class="mb-1 block text-[0.68rem] tracking-wide text-ink-faint uppercase">
            {t("dialog.project.edit.name")}
          </span>
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
            {t("common.cancel")}
          </button>
          <button
            class="rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-accent-ink"
            onClick={() => void save()}
          >
            {t("common.save")}
          </button>
        </div>
      </div>
    </div>
  )
}

function MenuItem(props: { label: string; danger?: boolean; disabled?: boolean; onClick: () => void }) {
  return (
    <button
      class="w-full rounded-md px-2 py-1.5 text-left text-sm transition-colors"
      classList={{
        "text-ink-muted hover:bg-raised hover:text-ink": !props.danger && !props.disabled,
        "text-danger hover:bg-danger/10": props.danger && !props.disabled,
        "cursor-default text-ink-faint": props.disabled,
      }}
      disabled={props.disabled}
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

function ago(timestamp: number) {
  const seconds = Math.max(0, (Date.now() - timestamp) / 1000)
  if (seconds < 60) return t("common.time.justNow")
  if (seconds < 3600) return t("common.time.minutesAgo.short", { count: Math.floor(seconds / 60) })
  if (seconds < 86400) return t("common.time.hoursAgo.short", { count: Math.floor(seconds / 3600) })
  return t("common.time.daysAgo.short", { count: Math.floor(seconds / 86400) })
}
