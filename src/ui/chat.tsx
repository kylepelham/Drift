import type { SessionStatus } from "@opencode-ai/sdk/client"
import { batch, createEffect, createMemo, createSignal, For, on, onCleanup, Show, untrack } from "solid-js"
import { useEngine } from "../engine"
import type { MessageEntry } from "../engine/store"
import { t } from "../state/i18n"
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
    const sorted = [...(engine.state.transcripts[id] ?? [])]
      .filter((entry) => !revertedAt || entry.info.id < revertedAt)
      .sort((a, b) => a.info.id.localeCompare(b.info.id))
    return mergeCompactionEntries(sorted)
  })
  const sessionError = createMemo(() => {
    const id = selectedSession()
    if (!id) return null
    const error = engine.state.errors[id]
    if (!error) return null
    const latest = entries().at(-1)
    if (latest?.info.role !== "assistant") return error
    const messageError = (latest.info as { error?: { name: string; data?: unknown } }).error
    return messageError && messageError.name !== "MessageAbortedError" ? null : error
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
  const thinking = createMemo(() => {
    const id = selectedSession()
    return id ? thinkingState(entries(), engine.state.status[id]?.type) : null
  })
  const retry = createMemo(() => {
    const id = selectedSession()
    const status = id ? engine.state.status[id] : undefined
    return status?.type === "retry" ? status : undefined
  })

  const observer = new ResizeObserver((observations) => {
    let deltaAbove = 0
    let changed = false
    const viewportTop = scroller.getBoundingClientRect().top
    for (const observation of observations) {
      const row = observation.target as HTMLElement
      const id = row.dataset.mid
      if (!id) continue
      const next = observation.borderBoxSize[0]?.blockSize ?? row.offsetHeight
      if (next === 0) continue
      const previous = heights.get(id) ?? estimatedRow
      if (Math.abs(next - previous) < 1) continue
      heights.set(id, next)
      changed = true
      deltaAbove += resizeCompensation(previous, next, row.getBoundingClientRect().bottom, viewportTop)
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

  // Untracked stick: content growth follows the bottom, but flipping stick on its own
  // never scrolls, so easing into the stick zone by hand cannot yank the view.
  createEffect(() => {
    const last = entries().at(-1)
    transcriptRevision(last)
    offsets()
    sessionError()
    thinking()
    if (untrack(stick)) queueMicrotask(() => scroller.scrollTo({ top: scroller.scrollHeight }))
  })

  // Only user gestures may change stickiness; programmatic snaps, browser clamps, and
  // measurement churn fire scroll events too and used to unstick mid-settle.
  let gestureAt = 0
  let dragging = false
  let forwardedTarget: number | null = null
  let forwardedReset: ReturnType<typeof setTimeout> | undefined
  const gesture = () => (gestureAt = Date.now())
  const nativeWheel = () => {
    forwardedTarget = null
    clearTimeout(forwardedReset)
    gesture()
  }
  const releaseDrag = () => (dragging = false)
  const forwardedWheel = (event: Event) => {
    const detail = (event as CustomEvent<ForwardedWheel>).detail
    gesture()
    const delta = normalizedWheelDelta(detail.deltaY, detail.deltaMode, scroller.clientHeight)
    const max = Math.max(0, scroller.scrollHeight - scroller.clientHeight)
    forwardedTarget = accumulatedWheelTarget(scroller.scrollTop, forwardedTarget, delta, max)
    scroller.scrollTo({ top: forwardedTarget, behavior: "smooth" })
    clearTimeout(forwardedReset)
    forwardedReset = setTimeout(() => (forwardedTarget = null), 180)
  }
  window.addEventListener("pointerup", releaseDrag)
  window.addEventListener(chatWheelEvent, forwardedWheel)
  onCleanup(() => {
    clearTimeout(forwardedReset)
    window.removeEventListener("pointerup", releaseDrag)
    window.removeEventListener(chatWheelEvent, forwardedWheel)
  })

  function onScroll() {
    const top = scroller.scrollTop
    const previous = untrack(viewTop)
    setViewTop(top)
    setViewHeight(scroller.clientHeight)
    if (dragging || Date.now() - gestureAt < 250) {
      const distance = scroller.scrollHeight - top - scroller.clientHeight
      setStick(scrollGestureSticks(previous, top, distance))
    }
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
    <div
      ref={scroller}
      class="min-h-0 flex-1 overflow-x-hidden overflow-y-auto"
      onScroll={onScroll}
      onWheel={nativeWheel}
      onPointerDown={(event) => {
        gesture()
        dragging = event.target === scroller
      }}
      onTouchStart={gesture}
    >
      <Show when={selectedSession()} keyed fallback={<EmptyState />}>
        <div class="fade-in relative mx-auto box-content max-w-3xl px-4 pt-14 pb-6 select-text">
          <div style={{ height: `${offsets().at(-1)}px` }}>
            <div style={{ transform: `translateY(${offsets()[range().start]}px)` }}>
              <For each={slice()}>
                {(entry, index) => (
                  <Row
                    entry={entry}
                    next={slice()[index() + 1]}
                    thinking={thinking()?.messageID === entry.info.id && !retry()}
                    thinkingHeading={thinking()?.heading}
                    retry={thinking()?.messageID === entry.info.id ? retry() : undefined}
                    measure={measureRow}
                  />
                )}
              </For>
            </div>
          </div>
          <Show when={sessionError()}>
            {(error) => (
              <div class="pb-6" role="alert">
                <div class="rounded-lg border border-danger/40 bg-danger/10 px-3 py-2 text-sm break-words text-danger">
                  {error()}
                </div>
              </div>
            )}
          </Show>
        </div>
      </Show>
    </div>
  )
}

type ForwardedWheel = { deltaY: number; deltaMode: number }
const chatWheelEvent = "drift:chat-wheel"

export function forwardWheelToChat(event: WheelEvent, boundary: HTMLElement) {
  if (event.ctrlKey || event.deltaY === 0 || wheelTargetConsumes(event.target, boundary, event.deltaY)) return false
  window.dispatchEvent(
    new CustomEvent<ForwardedWheel>(chatWheelEvent, {
      detail: { deltaY: event.deltaY, deltaMode: event.deltaMode },
    }),
  )
  event.preventDefault()
  return true
}

function wheelTargetConsumes(target: EventTarget | null, boundary: HTMLElement, deltaY: number) {
  let element = target instanceof Element ? target : null
  while (element) {
    if (element.hasAttribute("data-wheel-lock")) return true
    if (element !== boundary) {
      const style = getComputedStyle(element)
      const scrollable = style.overflowY === "auto" || style.overflowY === "scroll"
      if (scrollable && element.scrollHeight > element.clientHeight) {
        const remaining = element.scrollHeight - element.clientHeight - element.scrollTop
        if ((deltaY < 0 && element.scrollTop > 0) || (deltaY > 0 && remaining > 1)) return true
      }
    }
    if (element === boundary) break
    element = element.parentElement
  }
  return false
}

export function normalizedWheelDelta(deltaY: number, deltaMode: number, viewportHeight: number) {
  if (deltaMode === 1) return deltaY * 16
  if (deltaMode === 2) return deltaY * viewportHeight
  return deltaY
}

export function accumulatedWheelTarget(scrollTop: number, pendingTarget: number | null, delta: number, max: number) {
  return Math.min(max, Math.max(0, (pendingTarget ?? scrollTop) + delta))
}

export function resizeCompensation(previous: number, next: number, rowBottom: number, viewportTop: number) {
  return rowBottom < viewportTop ? next - previous : 0
}

export function scrollGestureSticks(previousTop: number, nextTop: number, distanceFromBottom: number) {
  if (nextTop < previousTop) return false
  return distanceFromBottom < 80
}

type RevisionPart = {
  type: string
  text?: string
  state?: { status?: string; output?: unknown; error?: unknown; metadata?: Record<string, unknown> }
  metadata?: Record<string, unknown>
}

export function transcriptRevision(entry?: { parts: RevisionPart[] }) {
  if (!entry) return "0"
  let revision = `${entry.parts.length}`
  for (const part of entry.parts) {
    if (part.type === "text" || part.type === "reasoning") {
      revision += `|${part.type}:${part.text?.length ?? 0}`
      continue
    }
    if (part.type !== "tool") continue
    const state = part.state
    revision += `|tool:${state?.status ?? ""}`
    if (typeof state?.output === "string") revision += `:o${state.output.length}`
    if (typeof state?.error === "string") revision += `:e${state.error.length}`
    const metadata = state?.metadata ?? part.metadata
    if (typeof metadata?.output === "string") revision += `:m${metadata.output.length}`
    if (typeof metadata?.diff === "string") revision += `:d${metadata.diff.length}`
  }
  return revision
}

export function mergeCompactionEntries(entries: MessageEntry[]) {
  return entries.filter((entry, index) => {
    const next = entries[index + 1]
    const boundary =
      entry.info.role === "user" &&
      entry.parts.some((part) => part.type === "compaction") &&
      entry.parts.every((part) => part.type === "compaction" || (part.type === "text" && part.synthetic))
    const summary =
      next?.info.role === "assistant" &&
      !!(next.info as { summary?: boolean }).summary &&
      "parentID" in next.info &&
      next.info.parentID === entry.info.id
    return !boundary || !summary
  })
}

export function thinkingAfterMessage(entries: MessageEntry[], status?: string) {
  return thinkingState(entries, status)?.messageID ?? null
}

export function thinkingState(entries: MessageEntry[], status?: string) {
  if (status !== "busy" && status !== "retry") return null
  const newestFirst = [...entries].reverse()
  const unfinished = newestFirst.find(
    (entry) => entry.info.role === "assistant" && !(entry.info as { time: { completed?: number } }).time.completed,
  )
  const parentID = unfinished && "parentID" in unfinished.info ? unfinished.info.parentID : undefined
  const activeUser =
    (parentID && entries.find((entry) => entry.info.role === "user" && entry.info.id === parentID)) ??
    newestFirst.find((entry) => entry.info.role === "user")
  if (!activeUser) return null
  const assistants = entries.filter(
    (entry) => entry.info.role === "assistant" && "parentID" in entry.info && entry.info.parentID === activeUser.info.id,
  )
  const error = assistants.find(
    (entry) =>
      (entry.info as { error?: { name?: string } }).error &&
      (entry.info as { error?: { name?: string } }).error?.name !== "MessageAbortedError",
  )
  if (status === "busy" && error) return null
  const heading = assistants
    .flatMap((entry) => entry.parts)
    .map((part) => (part.type === "reasoning" && part.text ? reasoningHeading(part.text) : undefined))
    .find((value): value is string => !!value)
  return { messageID: assistants.at(-1)?.info.id ?? activeUser.info.id, heading }
}

export function reasoningHeading(text: string) {
  const markdown = text.replace(/\r\n?/g, "\n")
  const html = markdown.match(/<h[1-6][^>]*>([\s\S]*?)<\/h[1-6]>/i)
  if (html?.[1]) {
    const value = cleanHeading(html[1].replace(/<[^>]+>/g, " "))
    if (value) return value
  }
  const atx = markdown.match(/^\s{0,3}#{1,6}[ \t]+(.+?)(?:[ \t]+#+[ \t]*)?$/m)
  if (atx?.[1]) {
    const value = cleanHeading(atx[1])
    if (value) return value
  }
  const setext = markdown.match(/^([^\n]+)\n(?:=+|-+)\s*$/m)
  if (setext?.[1]) {
    const value = cleanHeading(setext[1])
    if (value) return value
  }
  const strong = markdown.match(/^\s*(?:\*\*|__)(.+?)(?:\*\*|__)\s*$/m)
  if (strong?.[1]) return cleanHeading(strong[1]) || undefined
}

function cleanHeading(value: string) {
  return value
    .replace(/`([^`]+)`/g, "$1")
    .replace(/\[([^\]]+)\]\([^)]+\)/g, "$1")
    .replace(/[*_~]+/g, "")
    .replace(/\s+/g, " ")
    .trim()
}

function Row(props: {
  entry: MessageEntry
  next?: MessageEntry
  thinking: boolean
  thinkingHeading?: string
  retry?: Extract<SessionStatus, { type: "retry" }>
  measure: (element: HTMLDivElement) => void
}) {
  const fresh = Date.now() - props.entry.info.time.created < 2000
  return (
    <div ref={props.measure} data-mid={props.entry.info.id} class="min-w-0 max-w-full pb-6" classList={{ "fade-up": fresh }}>
      <MessageView entry={props.entry} footer={props.next?.info.role !== "assistant"} />
      <Show when={props.thinking}>
        <div class="timeline-thinking select-none" role="status" aria-live="polite">
          <TextShimmer text={t("drift.chat.thinking")} />
          <Show when={props.thinkingHeading}>{(heading) => <span class="timeline-thinking-heading">{heading()}</span>}</Show>
        </div>
      </Show>
      <Show when={props.retry}>{(status) => <SessionRetry status={status()} />}</Show>
    </div>
  )
}

function SessionRetry(props: { status: Extract<SessionStatus, { type: "retry" }> }) {
  const [now, setNow] = createSignal(Date.now())
  const timer = setInterval(() => setNow(Date.now()), 1000)
  onCleanup(() => clearInterval(timer))
  const display = createMemo(() => retryPresentation(props.status, now()))
  return (
    <div class="mt-3 rounded-lg border border-danger/40 bg-danger/10 px-3 py-2 text-sm text-danger" role="status" aria-live="polite">
      <div class="flex items-start gap-2">
        <span class="pulse-soft mt-1.5 size-2 shrink-0 rounded-full bg-danger" aria-hidden="true" />
        <div class="min-w-0">
          <div class="break-words" classList={{ "cursor-help": display().truncated }} title={display().truncated ? props.status.message : undefined}>
            {display().message}
          </div>
          <div class="mt-0.5 text-xs text-danger/75">{display().info}</div>
        </div>
      </div>
    </div>
  )
}

export function retryPresentation(status: Extract<SessionStatus, { type: "retry" }>, now: number) {
  const normalized = status.message.trim() || t("drift.chat.retry.providerRejected")
  const message = normalized.length > 80 ? normalized.slice(0, 80) + "..." : normalized
  const seconds = Math.max(0, Math.round((status.next - now) / 1000))
  const retry = seconds > 0 ? t("drift.chat.retry.inSeconds", { seconds }) : t("drift.chat.retry.now")
  return {
    message,
    truncated: normalized.length > 80,
    info: t("drift.chat.retry.info", { retry, attempt: status.attempt }),
  }
}

function EmptyState() {
  return (
    <div class="flex h-full flex-col items-center justify-center gap-3 select-none">
      <div class="fade-up text-4xl font-semibold tracking-tight text-ink">drift</div>
      <Show
        when={activeWorkspace()}
        fallback={
          <div class="fade-up text-sm text-ink-muted" style={{ "animation-delay": "80ms" }}>
            {t("drift.chat.empty.noWorkspace")}
          </div>
        }
      >
        <div class="fade-up text-sm text-ink-muted" style={{ "animation-delay": "80ms" }}>
          {t("drift.chat.empty.promptHint")}
        </div>
      </Show>
      <div class="fade-up text-xs text-ink-faint" style={{ "animation-delay": "160ms" }}>
        {t("drift.chat.empty.threadHint")}
      </div>
    </div>
  )
}
