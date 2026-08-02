import { createEffect, createMemo, createSignal, For, on, Show } from "solid-js"
import { useEngine } from "../engine"
import type { MessageEntry } from "../engine/store"
import { t } from "../state/i18n"
import { selectedSession } from "../state/selection"
import { Chevron } from "./controls"
import { restoreReverted, revertDockEntries, revertPreview } from "./revert"

export function RevertDock() {
  const engine = useEngine()
  const [open, setOpen] = createSignal(false)
  const [busy, setBusy] = createSignal(false)
  const items = createMemo(() => {
    const id = selectedSession()
    if (!id) return []
    return revertDockEntries(engine.state.transcripts[id] ?? [], engine.state.sessions[id]?.revert?.messageID)
  })
  createEffect(on(() => `${selectedSession()}|${items().length}|${items()[0]?.info.id ?? ""}`, () => setOpen(false)))

  async function restore(entry: MessageEntry) {
    const id = selectedSession()
    if (!id || busy()) return
    setBusy(true)
    try {
      await restoreReverted(engine, id, entry.info.id)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Show when={items().length > 0}>
      <div class="dock-card rounded-lg border border-edge bg-surface text-sm">
        <button
          class="flex w-full min-w-0 items-center gap-2 overflow-hidden whitespace-nowrap px-3 py-1.5 text-ink-muted"
          aria-expanded={open()}
          aria-label={open() ? t("session.revertDock.collapse") : t("session.revertDock.expand")}
          onClick={() => setOpen(!open())}
        >
          <Chevron open={open()} />
          <span class="shrink-0">
            {t(items().length === 1 ? "session.revertDock.summary.one" : "session.revertDock.summary.other", {
              count: items().length,
            })}
          </span>
          <Show when={!open()}>
            <span class="min-w-0 flex-1 truncate text-left text-ink-faint">{revertPreview(items()[0])}</span>
          </Show>
        </button>
        <Show when={open()}>
          <ul class="space-y-0.5 border-t border-edge px-2 py-2">
            <For each={items()}>
              {(entry) => (
                <li class="flex items-center gap-2 rounded-md px-2 py-1 text-xs text-ink-muted">
                  <span class="min-w-0 flex-1 truncate">{revertPreview(entry)}</span>
                  <button
                    class="shrink-0 rounded-md border border-edge px-2 py-0.5 transition-colors hover:border-accent/50 hover:text-ink disabled:cursor-default disabled:opacity-50"
                    disabled={busy()}
                    onClick={() => void restore(entry)}
                  >
                    {t("session.revertDock.restore")}
                  </button>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </div>
    </Show>
  )
}
