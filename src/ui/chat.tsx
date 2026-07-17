import { createEffect, createMemo, createSignal, For, on, Show } from "solid-js"
import { useEngine } from "../engine"
import { sessionBusy } from "../engine/store"
import { selectedSession } from "../state/selection"
import { MessageView } from "./message"

export function Chat() {
  const engine = useEngine()
  const entries = createMemo(() => {
    const id = selectedSession()
    if (!id) return []
    return [...(engine.state.transcripts[id] ?? [])].sort((a, b) => a.info.id.localeCompare(b.info.id))
  })

  createEffect(
    on(selectedSession, (id) => {
      if (id) void engine.actions.openSession(id)
    }),
  )

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
        <div class="mx-auto flex max-w-3xl flex-col gap-5 px-4 py-6">
          <For each={entries()}>{(entry) => <MessageView entry={entry} />}</For>
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
      <div class="fade-up text-sm text-ink-muted" style={{ "animation-delay": "80ms" }}>
        Start typing below. Enter sends, Shift+Enter breaks the line.
      </div>
      <div class="fade-up text-xs text-ink-faint" style={{ "animation-delay": "160ms" }}>
        Threads live on the left. No tabs. Ever.
      </div>
    </div>
  )
}
