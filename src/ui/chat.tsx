import { createEffect, createMemo, createSignal, For, Show } from "solid-js"
import { useEngine } from "../engine"
import { sessionBusy } from "../engine/store"
import { selectedSession } from "../state/selection"
import { activeWorkspace } from "../state/workspaces"
import { MessageView } from "./message"

export function Chat() {
  const engine = useEngine()
  const entries = createMemo(() => {
    const id = selectedSession()
    if (!id) return []
    const revertedAt = engine.state.sessions[id]?.revert?.messageID
    return [...(engine.state.transcripts[id] ?? [])]
      .filter((entry) => !revertedAt || entry.info.id < revertedAt)
      .sort((a, b) => a.info.id.localeCompare(b.info.id))
  })

  createEffect(() => {
    const id = selectedSession()
    const known = !!id && !!engine.state.sessions[id]
    if (known && engine.state.connection === "online") void engine.actions.openSession(id)
  })

  let scroller!: HTMLDivElement
  const [stick, setStick] = createSignal(true)
  createEffect(() => {
    const last = entries().at(-1)
    if (last) JSON.stringify(last.parts)
    if (stick()) queueMicrotask(() => scroller.scrollTo({ top: scroller.scrollHeight }))
  })

  const busy = () => {
    const id = selectedSession()
    return !!id && sessionBusy(engine.state, id)
  }

  return (
    <div
      ref={scroller}
      class="min-h-0 flex-1 overflow-y-auto"
      onScroll={() => setStick(scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 80)}
    >
      <Show when={selectedSession()} fallback={<EmptyState />}>
        <div class="mx-auto flex max-w-3xl flex-col gap-5 px-4 py-6 select-text">
          <For each={entries()}>
            {(entry, index) => (
              <MessageView entry={entry} footer={entries()[index() + 1]?.info.role !== "assistant"} />
            )}
          </For>
          <Show when={busy()}>
            <div class="pulse-soft text-sm text-ink-faint">working...</div>
          </Show>
        </div>
      </Show>
    </div>
  )
}

function EmptyState() {
  return (
    <div class="flex h-full flex-col items-center justify-center gap-3 select-none">
      <div class="fade-up text-4xl font-semibold tracking-tight text-ink">drift</div>
      <Show
        when={activeWorkspace()}
        fallback={
          <div class="fade-up text-sm text-ink-muted" style={{ "animation-delay": "80ms" }}>
            Add or select a workspace on the left to start.
          </div>
        }
      >
        <div class="fade-up text-sm text-ink-muted" style={{ "animation-delay": "80ms" }}>
          Start typing below. Enter sends, Shift+Enter breaks the line.
        </div>
      </Show>
      <div class="fade-up text-xs text-ink-faint" style={{ "animation-delay": "160ms" }}>
        Threads live on the left. No tabs. Ever.
      </div>
    </div>
  )
}
