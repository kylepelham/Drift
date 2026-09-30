import { createSignal, onCleanup, onMount, Show } from "solid-js"
import { createStore } from "solid-js/store"
import { Portal } from "solid-js/web"
import { useEngine } from "../engine"
import type { BranchDraft } from "../engine/actions"
import { t } from "../state/i18n"
import { selectSession } from "../state/selection"
import { IconX } from "./icons"
import { activateModal, closeOnBackdropPointerDown } from "./modal"
import { TextShimmer } from "./text-shimmer"

type Request = { source: string; goal: string }

const [request, setRequest] = createSignal<Request | null>(null)

/** `/spawn <goal>`: draft a handoff from `source`, let the user edit it, then branch. */
export function openBranch(source: string, goal: string) {
  setRequest({ source, goal })
}

export function BranchHost() {
  return (
    <Show when={request()} keyed>
      {(current) => (
        <Portal>
          <BranchDialog request={current} onClose={() => setRequest(null)} />
        </Portal>
      )}
    </Show>
  )
}

function BranchDialog(props: { request: Request; onClose: () => void }) {
  const engine = useEngine()
  let dialog!: HTMLDivElement
  let open = true
  const [draft, setDraft] = createStore<{ value: BranchDraft | null }>({ value: null })
  const [creating, setCreating] = createSignal(false)
  onCleanup(() => (open = false))
  onMount(() => {
    onCleanup(activateModal(dialog, props.onClose))
    void engine.actions.draftBranch(props.request.source, props.request.goal).then((drafted) => {
      if (!open) return
      if (drafted) setDraft("value", drafted)
      else props.onClose()
    })
  })

  async function create() {
    const value = draft.value
    if (!value || !value.goal.trim() || creating()) return
    setCreating(true)
    const session = await engine.actions.branch(props.request.source, value)
    setCreating(false)
    if (!session) return
    selectSession(session.id)
    props.onClose()
  }

  const field = "w-full rounded-md border border-edge bg-surface px-2.5 py-1.5 text-sm text-ink outline-none focus:border-accent/60"
  const label = "mb-1 block text-xs font-medium text-ink-muted"
  return (
    <div
      data-modal-layer
      class="fixed inset-0 z-30 flex items-center justify-center bg-black/50 p-2 sm:p-4"
      onPointerDown={(event) => closeOnBackdropPointerDown(event, props.onClose, dialog)}
    >
      <div
        ref={dialog}
        role="dialog"
        aria-modal="true"
        aria-label={t("drift.branch.title")}
        tabIndex={-1}
        class="fade-up flex max-h-[calc(100vh-2rem)] w-[min(40rem,calc(100vw-1rem))] flex-col overflow-hidden rounded-xl border border-edge bg-overlay shadow-2xl shadow-black/40"
      >
        <div class="flex items-start justify-between border-b border-edge px-4 py-3">
          <div>
            <div class="text-sm font-semibold text-ink">{t("drift.branch.title")}</div>
            <div class="mt-0.5 text-xs text-ink-faint">{t("drift.branch.description")}</div>
          </div>
          <button
            title={t("common.close")}
            class="flex size-7 items-center justify-center rounded-md text-ink-faint hover:bg-raised hover:text-ink"
            onClick={props.onClose}
          >
            <IconX />
          </button>
        </div>
        <Show
          when={draft.value}
          fallback={
            <div class="px-4 py-8 text-sm text-ink-muted">
              <TextShimmer text={t("drift.branch.drafting")} />
            </div>
          }
        >
          {(value) => (
            <>
              <div class="min-h-0 flex-1 space-y-3 overflow-y-auto p-4">
                <div>
                  <label class={label} for="branch-title">{t("drift.branch.name")}</label>
                  <input id="branch-title" class={field} value={value().title} onInput={(e) => setDraft("value", "title", e.currentTarget.value)} />
                </div>
                <div>
                  <label class={label} for="branch-goal">{t("drift.branch.goal")}</label>
                  <textarea id="branch-goal" rows={2} class={field} value={value().goal} onInput={(e) => setDraft("value", "goal", e.currentTarget.value)} />
                </div>
                <div>
                  <label class={label} for="branch-summary">{t("drift.branch.summary")}</label>
                  <textarea id="branch-summary" rows={8} class={field} value={value().summary} onInput={(e) => setDraft("value", "summary", e.currentTarget.value)} />
                </div>
                <div>
                  <label class={label} for="branch-excerpts">{t("drift.branch.excerpts")}</label>
                  <textarea id="branch-excerpts" rows={4} class={`${field} font-mono text-xs`} value={value().excerpts} onInput={(e) => setDraft("value", "excerpts", e.currentTarget.value)} />
                </div>
              </div>
              <div class="flex justify-end gap-2 border-t border-edge px-4 py-3">
                <button class="rounded-md px-3 py-1.5 text-sm text-ink-muted hover:bg-raised hover:text-ink" onClick={props.onClose}>
                  {t("common.cancel")}
                </button>
                <button
                  class="rounded-md bg-accent px-3 py-1.5 text-sm font-medium text-white disabled:opacity-50"
                  disabled={creating() || !value().goal.trim()}
                  onClick={() => void create()}
                >
                  {t("drift.branch.create")}
                </button>
              </div>
            </>
          )}
        </Show>
      </div>
    </div>
  )
}
