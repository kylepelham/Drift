import { createEffect, For, onCleanup, onMount, Show, type JSX } from "solid-js"
import { Portal } from "solid-js/web"
import { useEngine } from "../engine"
import { t } from "../state/i18n"
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
import { activateModal, closeOnBackdropPointerDown } from "./modal"
import { WorkspaceIcon } from "./workspaces"

const purgeAge = 7 * 24 * 60 * 60 * 1000

export function ArchiveModal(props: { onClose: () => void }) {
  const engine = useEngine()
  let dialog!: HTMLDivElement
  const allWorkspaces = () => [...workspaces(), ...removedWorkspaces()]
  onMount(() => onCleanup(activateModal(dialog, props.onClose)))
  createEffect(() => {
    if (engine.state.connection !== "online") return
    void engine.actions.loadAllSessions()
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
    <Portal>
      <div
        data-modal-layer
        class="fixed inset-0 z-30 flex items-center justify-center bg-black/50"
        onPointerDown={(event) => closeOnBackdropPointerDown(event, props.onClose, dialog)}
      >
        <div
          ref={dialog}
          role="dialog"
          aria-modal="true"
          aria-label={t("drift.sidebar.archived")}
          tabIndex={-1}
          class="fade-up flex max-h-[70vh] w-[32rem] flex-col overflow-hidden rounded-xl border border-edge bg-overlay shadow-2xl shadow-black/40"
          onClick={(event) => event.stopPropagation()}
        >
          <div class="flex items-center justify-between border-b border-edge px-4 py-3">
            <div>
              <div class="text-sm font-semibold text-ink">{t("drift.sidebar.archived")}</div>
              <div class="mt-0.5 text-xs text-ink-faint">{t("drift.archive.retention")}</div>
            </div>
            <button
              title={t("common.close")}
              class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
              onClick={props.onClose}
            >
              <IconX />
            </button>
          </div>
          <div class="min-h-0 flex-1 space-y-5 overflow-y-auto p-4">
            <Show when={removedWorkspaces().length > 0}>
              <ArchiveSection title={t("drift.sidebar.workspaces")}>
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
              <ArchiveSection title={t("drift.archive.threads")}>
                <For each={archivedSessions()}>
                  {(entry) => {
                    const workspace = () => allWorkspaces().find((item) => item.id === entry.workspaceId)
                    const session = () => engine.state.sessions[entry.sessionId]
                    return (
                      <ArchiveRow
                        title={session()?.title || t("drift.thread.untitled")}
                        detail={`${workspace()?.name ?? t("drift.archive.unknownWorkspace")} · ${expiry(entry.archivedAt)}`}
                        onRestore={() => void restoreThread(entry.sessionId, entry.workspaceId)}
                      />
                    )
                  }}
                </For>
              </ArchiveSection>
            </Show>
            <Show when={removedWorkspaces().length === 0 && archivedSessions().length === 0}>
              <div class="py-10 text-center text-sm text-ink-faint">{t("drift.archive.empty")}</div>
            </Show>
          </div>
        </div>
      </div>
    </Portal>
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
        title={t("drift.titlebar.restore")}
        class="flex size-8 shrink-0 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-overlay hover:text-ink"
        onClick={props.onRestore}
      >
        <IconRestore />
      </button>
    </div>
  )
}

function expiry(timestamp?: number) {
  if (!timestamp) return t("drift.archive.deletesSoon")
  const days = Math.max(1, Math.ceil((timestamp + purgeAge - Date.now()) / (24 * 60 * 60 * 1000)))
  return t("drift.archive.deletesInDays", { days })
}
