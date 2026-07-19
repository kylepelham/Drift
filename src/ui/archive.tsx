import { createEffect, For, onCleanup, Show, type JSX } from "solid-js"
import { useEngine } from "../engine"
import { selectSession } from "../state/selection"
import {
  archivedSessions,
  removedWorkspaces,
  restoreWorkspace,
  selectWorkspace,
  unarchiveSession,
  workspaces,
} from "../state/workspaces"
import { IconRestore, IconX } from "./icons"
import { WorkspaceIcon } from "./workspaces"

const purgeAge = 7 * 24 * 60 * 60 * 1000

export function ArchiveModal(props: { onClose: () => void }) {
  const engine = useEngine()
  const allWorkspaces = () => [...workspaces(), ...removedWorkspaces()]

  createEffect(() => {
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") props.onClose()
    }
    document.addEventListener("keydown", escape)
    onCleanup(() => document.removeEventListener("keydown", escape))
  })
  createEffect(() => {
    if (engine.state.connection !== "online") return
    for (const workspace of removedWorkspaces()) void engine.actions.loadSessions(workspace.path)
  })

  async function restoreThread(sessionId: string, workspaceId: string) {
    const workspace = allWorkspaces().find((entry) => entry.id === workspaceId)
    if (workspace?.removedAt) await restoreWorkspace(workspace)
    else if (workspace) selectWorkspace(workspace.id)
    await unarchiveSession(sessionId)
    selectSession(sessionId)
    props.onClose()
  }

  return (
    <div class="fixed inset-0 z-30 flex items-center justify-center bg-black/50" onClick={props.onClose}>
      <div
        class="fade-up flex max-h-[70vh] w-[32rem] flex-col overflow-hidden rounded-xl border border-edge bg-overlay shadow-2xl shadow-black/40"
        onClick={(event) => event.stopPropagation()}
      >
        <div class="flex items-center justify-between border-b border-edge px-4 py-3">
          <div>
            <div class="text-sm font-semibold text-ink">Archived</div>
            <div class="mt-0.5 text-xs text-ink-faint">Items are permanently removed after seven days.</div>
          </div>
          <button
            title="Close"
            class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
            onClick={props.onClose}
          >
            <IconX />
          </button>
        </div>
        <div class="min-h-0 flex-1 space-y-5 overflow-y-auto p-4">
          <Show when={removedWorkspaces().length > 0}>
            <ArchiveSection title="Workspaces">
              <For each={removedWorkspaces()}>
                {(workspace) => (
                  <ArchiveRow
                    title={workspace.name}
                    detail={`${workspace.path} · ${expiry(workspace.removedAt)}`}
                    icon={<WorkspaceIcon workspace={workspace} />}
                    onRestore={() => void restoreWorkspace(workspace).then(props.onClose)}
                  />
                )}
              </For>
            </ArchiveSection>
          </Show>
          <Show when={archivedSessions().length > 0}>
            <ArchiveSection title="Threads">
              <For each={archivedSessions()}>
                {(entry) => {
                  const workspace = () => allWorkspaces().find((item) => item.id === entry.workspaceId)
                  const session = () => engine.state.sessions[entry.sessionId]
                  return (
                    <ArchiveRow
                      title={session()?.title || "Untitled thread"}
                      detail={`${workspace()?.name ?? "Unknown workspace"} · ${expiry(entry.archivedAt)}`}
                      onRestore={() => void restoreThread(entry.sessionId, entry.workspaceId)}
                    />
                  )
                }}
              </For>
            </ArchiveSection>
          </Show>
          <Show when={removedWorkspaces().length === 0 && archivedSessions().length === 0}>
            <div class="py-10 text-center text-sm text-ink-faint">Nothing archived.</div>
          </Show>
        </div>
      </div>
    </div>
  )
}

function ArchiveSection(props: { title: string; children: JSX.Element }) {
  return (
    <section>
      <div class="mb-1.5 text-[0.68rem] tracking-wider text-ink-faint uppercase">{props.title}</div>
      <div class="space-y-1">{props.children}</div>
    </section>
  )
}

function ArchiveRow(props: { title: string; detail: string; icon?: JSX.Element; onRestore: () => void }) {
  return (
    <div class="flex items-center gap-3 rounded-lg border border-transparent px-3 py-2.5 hover:border-edge hover:bg-raised/50">
      {props.icon}
      <div class="min-w-0 flex-1">
        <div class="truncate text-sm text-ink">{props.title}</div>
        <div class="truncate text-xs text-ink-faint">{props.detail}</div>
      </div>
      <button
        title="Restore"
        class="flex size-8 shrink-0 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-overlay hover:text-ink"
        onClick={props.onRestore}
      >
        <IconRestore />
      </button>
    </div>
  )
}

function expiry(timestamp?: number) {
  if (!timestamp) return "Deletes soon"
  const days = Math.max(1, Math.ceil((timestamp + purgeAge - Date.now()) / (24 * 60 * 60 * 1000)))
  return `Deletes in ${days}d`
}
