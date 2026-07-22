import { batch, createEffect, createMemo, createSignal, For, on, onCleanup, Show, untrack } from "solid-js"
import { useEngine } from "../engine"
import type { MessageEntry } from "../engine/store"
import { selectedSession } from "../state/selection"
import { activeWorkspace } from "../state/workspaces"
import { MessageView } from "./message"
import { TextShimmer } from "./text-shimmer"

const estimatedRow = 96
const overscan = 800
const loadOlderAt = 1200

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
  const [viewTop, setViewTop] = createSignal(0)
  const [viewHeight, setViewHeight] = createSignal(800)
  const heights = new Map<string, number>()
  const [measured, setMeasured] = createSignal(0)
  let loadingOlder = false

  const offsets = createMemo(() => {
    measured()
    const list = entries()
    const result = new Array<number>(list.length + 1)
    result[0] = 0
    for (let index = 0; index < list.length; index++)
      result[index + 1] = result[index] + (heights.get(list[index].info.id) ?? estimatedRow)
    return result
  })

  const range = createMemo(() => {
    const list = offsets()
    const top = viewTop() - overscan
    const bottom = viewTop() + viewHeight() + overscan
    let start = 0
    while (start < list.length - 1 && list[start + 1] < top) start++
    let end = start
    while (end < list.length - 1 && list[end] < bottom) end++
    return { start, end }
  })

  const slice = createMemo(() => entries().slice(range().start, range().end))
  const thinkingAfter = createMemo(() => {
    const id = selectedSession()
    return id ? thinkingAfterMessage(entries(), engine.state.status[id]?.type) : null
  })

  const observer = new ResizeObserver((observations) => {
    let deltaAbove = 0
    let changed = false
    const list = untrack(entries)
    const offs = untrack(offsets)
    const top = untrack(viewTop)
    for (const observation of observations) {
      const id = (observation.target as HTMLElement).dataset.mid
      if (!id) continue
      const next = observation.borderBoxSize[0]?.blockSize ?? (observation.target as HTMLElement).offsetHeight
      if (next === 0) continue
      const previous = heights.get(id) ?? estimatedRow
      if (Math.abs(next - previous) < 1) continue
      heights.set(id, next)
      changed = true
      const index = list.findIndex((entry) => entry.info.id === id)
      if (index >= 0 && offs[index + 1] < top) deltaAbove += next - previous
    }
    if (!changed) return
    setMeasured((value) => value + 1)
    if (deltaAbove !== 0 && !untrack(stick)) scroller.scrollTop += deltaAbove
  })
  onCleanup(() => observer.disconnect())

  function measureRow(element: HTMLDivElement) {
    observer.observe(element)
  }

  createEffect(on(selectedSession, () => {
    heights.clear()
    batch(() => {
      setMeasured((value) => value + 1)
      setStick(true)
      setViewTop(0)
    })
  }))

  createEffect(() => {
    const last = entries().at(-1)
    if (last) JSON.stringify(last.parts)
    offsets()
    if (stick()) queueMicrotask(() => scroller.scrollTo({ top: scroller.scrollHeight }))
  })

  function onScroll() {
    const top = scroller.scrollTop
    setViewTop(top)
    setViewHeight(scroller.clientHeight)
    setStick(scroller.scrollHeight - top - scroller.clientHeight < 80)
    maybeLoadOlder(top)
  }

  function maybeLoadOlder(top: number) {
    const id = selectedSession()
    if (!id || loadingOlder || top > loadOlderAt || !engine.state.cursors[id]) return
    loadingOlder = true
    const before = scroller.scrollHeight - scroller.scrollTop
    void engine.actions.loadOlder(id).finally(() => {
      queueMicrotask(() => {
        scroller.scrollTop = scroller.scrollHeight - before
        loadingOlder = false
      })
    })
  }

  return (
    <div ref={scroller} class="min-h-0 flex-1 overflow-x-hidden overflow-y-auto" onScroll={onScroll}>
      <Show when={selectedSession()} keyed fallback={<EmptyState />}>
        <div
          class="fade-in relative mx-auto box-content max-w-3xl px-4 pt-14 pb-6 select-text"
          style={{ height: `${offsets().at(-1)}px` }}
        >
          <div style={{ transform: `translateY(${offsets()[range().start]}px)` }}>
            <For each={slice()}>
              {(entry, index) => (
                <Row
                  entry={entry}
                  next={slice()[index() + 1]}
                  thinking={thinkingAfter() === entry.info.id}
                  measure={measureRow}
                />
              )}
            </For>
          </div>
        </div>
      </Show>
    </div>
  )
}

export function thinkingAfterMessage(entries: MessageEntry[], status?: string) {
  if (status !== "busy") return null
  const newestFirst = [...entries].reverse()
  const unfinished = newestFirst.find(
    (entry) => entry.info.role === "assistant" && !(entry.info as { time: { completed?: number } }).time.completed,
  )
  const parentID = unfinished && "parentID" in unfinished.info ? unfinished.info.parentID : undefined
  const activeUser =
    (parentID && entries.find((entry) => entry.info.role === "user" && entry.info.id === parentID)) ??
    newestFirst.find((entry) => entry.info.role === "user")
  if (!activeUser) return null
  const assistant = newestFirst.find(
    (entry) => entry.info.role === "assistant" && "parentID" in entry.info && entry.info.parentID === activeUser.info.id,
  )
  return assistant?.info.id ?? activeUser.info.id
}

function Row(props: {
  entry: MessageEntry
  next?: MessageEntry
  thinking: boolean
  measure: (element: HTMLDivElement) => void
}) {
  const fresh = Date.now() - props.entry.info.time.created < 2000
  return (
    <div ref={props.measure} data-mid={props.entry.info.id} class="min-w-0 max-w-full pb-6" classList={{ "fade-up": fresh }}>
      <MessageView entry={props.entry} footer={props.next?.info.role !== "assistant"} />
      <Show when={props.thinking}>
        <div class="timeline-thinking" role="status" aria-live="polite">
          <TextShimmer text="Thinking" />
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
