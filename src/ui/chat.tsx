import type { AssistantMessage, Part, SessionStatus } from "@opencode-ai/sdk/client"
import { batch, createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show, untrack } from "solid-js"
import { useEngine } from "../engine"
import { compareMessages, messageRevisionKey, messageText, modelInfo, type MessageEntry, type ModelRef } from "../engine/store"
import { codeFontSize } from "../state/code"
import { t } from "../state/i18n"
import { selectedSession } from "../state/selection"
import { activeWorkspace } from "../state/workspaces"
import { IconArrowDown } from "./icons"
import {
  assistantFlowContinues,
  groupAssistantEntries,
  largeUserText,
  MessageView,
  messageVisible,
  type PartGroup,
} from "./message"
import { TextShimmer } from "./text-shimmer"
import { DriftLogo } from "./logo"
import { recoverableForSession, recoverableInterruptions } from "../state/recovery"
import { RecoveryCard, retryModelItems } from "./recovery"
import { interruptResponseAnimations } from "./response-animation"
import { collapseCompaction, compactionCollapsed, prefsFor, updatePrefs } from "../state/prefs"
import { Picker } from "./picker"
import { ProviderIcon } from "./provider-icon"

const estimatedRow = 96
const overscan = 800
const loadOlderAt = 1200
// Within this distance of the bottom the view is considered "at the bottom": it keeps auto-scrolling
// with new output and hides the jump-to-latest button. scrollGestureSticks and
// shouldShowScrollToBottom are complementary halves of that decision and must share the threshold.
const stickyThresholdPx = 80
// A wheel gesture is treated as still in progress for this long after the last event, so momentum
// scrolling does not get mistaken for the user settling on a position.
const gestureWindowMs = 250
// Messages younger than this are treated as newly arrived rather than restored history.
const freshMessageMs = 2000
const maxRetryMessageChars = 80

export function Chat() {
  const engine = useEngine()
  const entries = createMemo(() => {
    const id = selectedSession()
    if (!id) return []
    const revertedAt = engine.state.sessions[id]?.revert?.messageID
    const transcript = engine.state.transcripts[id] ?? []
    // Nested part replacement does not invalidate every memo that iterates the transcript proxy.
    // Event reducers already bump this key for every message/part update, so consume it at the
    // transcript derivation boundary before persistent assistant slots reconcile their source.
    for (const entry of transcript) engine.state.revisions[messageRevisionKey(id, entry.info.id)]
    const boundary = revertedAt ? transcript.find((entry) => entry.info.id === revertedAt) : undefined
    const sorted = [...transcript]
      .filter((entry) => {
        if (!revertedAt) return true
        if (boundary) return compareMessages(entry, boundary) < 0
        return entry.info.id < revertedAt
      })
      .sort(compareMessages)
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
  const recoverable = createMemo(() => {
    recoverableInterruptions()
    const id = selectedSession()
    return id ? recoverableForSession(id) : undefined
  })
  const thinking = createMemo(() => {
    const id = selectedSession()
    return id ? thinkingState(entries(), engine.state.status[id]?.type) : null
  })
  const retry = createMemo(() => {
    const id = selectedSession()
    const status = id ? engine.state.status[id] : undefined
    return status?.type === "retry" ? status : undefined
  })
  const timelineSource = createMemo(() => timelineEntries(entries(), thinking()?.messageID))
  const assistantGroups = createMemo(() => groupAssistantEntries(timelineSource()))
  const timeline = createMemo(() => {
    const source = timelineSource()
    const groups = assistantGroups()
    const active = thinking()?.messageID
    return source.filter((entry, index) => timelineRowVisible(entry, groups.get(entry.info.id), source[index + 1], active))
  })
  const nextEntries = createMemo(() => {
    const list = timeline()
    return new Map(list.map((entry, index) => [entry.info.id, list[index + 1]]))
  })
  const thinkingOnly = (entry?: MessageEntry) =>
    !!entry && thinking()?.messageID === entry.info.id && !messageVisible(entry)
  const collapsedSummary = (entry: MessageEntry) =>
    entry.info.role === "assistant" && !!(entry.info as AssistantMessage).summary && collapseCompaction() && compactionCollapsed()

  createEffect(() => {
    const id = selectedSession()
    const known = !!id && !!engine.state.sessions[id]
    if (known && engine.state.connection === "online") void engine.actions.openSession(id)
  })

  // A revert that spans more than one transcript page can put every loaded message inside the
  // reverted range, leaving the timeline empty (the marker itself may not even be loaded). Page
  // older history in until something pre-revert is visible or the history is exhausted. The
  // in-flight signal re-runs this effect when each page lands, so the loop advances one page at
  // a time and stops the moment an entry survives the revert filter.
  const [revertBackfill, setRevertBackfill] = createSignal(false)
  createEffect(() => {
    const id = selectedSession()
    if (!id || revertBackfill()) return
    if (
      !revertBackfillNeeded({
        revertedAt: engine.state.sessions[id]?.revert?.messageID,
        visible: entries().length,
        loaded: engine.state.loaded[id],
        cursor: engine.state.cursors[id],
      })
    )
      return
    setRevertBackfill(true)
    void engine.actions
      .loadOlder(id)
      .catch(() => undefined)
      .finally(() => setRevertBackfill(false))
  })

  let scroller!: HTMLDivElement
  const [stick, setStick] = createSignal(true)
  const [awayFromBottom, setAwayFromBottom] = createSignal(false)
  const [viewTop, setViewTop] = createSignal(0)
  const [viewHeight, setViewHeight] = createSignal(800)
  const heights = new Map<string, number>()
  // Every part delta rebuilds offsets; re-estimating the text of hundreds of unmounted rows per
  // delta would burn a visible slice of a core, so estimates are cached per message revision.
  const estimates = new Map<string, { rev?: number; fontSize: number; thinking: boolean; collapsed: boolean; value: number }>()
  const [measured, setMeasured] = createSignal(0)
  let loadingOlder = false

  function rowEstimate(entry: MessageEntry, parts: Part[], fontSize: number, thinking: boolean, collapsed: boolean) {
    const rev = engine.state.revisions[messageRevisionKey(entry.info.sessionID, entry.info.id)]
    const cached = estimates.get(entry.info.id)
    if (cached && cached.rev === rev && cached.fontSize === fontSize && cached.thinking === thinking && cached.collapsed === collapsed)
      return cached.value
    const value = estimatedTimelineRow(entry, fontSize, parts, thinking, collapsed)
    estimates.set(entry.info.id, { rev, fontSize, thinking, collapsed, value })
    return value
  }

  const offsets = createMemo(() => {
    measured()
    const list = timeline()
    const fontSize = codeFontSize()
    const result = new Array<number>(list.length + 1)
    result[0] = 0
    const groups = assistantGroups()
    for (let index = 0; index < list.length; index++) {
      const entry = list[index]
      const parts = timelineParts(entry, groups.get(entry.info.id))
      result[index + 1] = result[index] +
        (heights.get(entry.info.id) ?? rowEstimate(entry, parts, fontSize, thinkingOnly(entry), collapsedSummary(entry)))
    }
    return result
  })

  const range = createMemo(() => {
    return virtualRange(offsets(), viewTop(), viewHeight())
  })

  const slice = createMemo(() => timeline().slice(range().start, range().end))

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
      const entry = untrack(timeline).find((item) => item.info.id === id)
      const parts = entry ? timelineParts(entry, untrack(assistantGroups).get(id)) : undefined
      const previous = heights.get(id) ??
        (entry
          ? estimatedTimelineRow(
            entry,
            untrack(codeFontSize),
            parts,
            untrack(() => thinkingOnly(entry)),
            untrack(() => collapsedSummary(entry)),
          )
          : estimatedRow)
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

  const viewportObserver = new ResizeObserver(() => {
    // Preserve the sticky bottom before publishing a resized viewport. Browser clamping can move
    // scrollTop when the composer/attention dock grows; recording that transient position makes
    // the virtual range jump before the queued follow correction runs.
    if (untrack(stick)) snapViewportToBottom()
    else publishViewport()
    const top = scroller.scrollTop
    if (untrack(stick)) return
    const distance = scroller.scrollHeight - top - scroller.clientHeight
    setAwayFromBottom(shouldShowScrollToBottom(distance))
  })
  onMount(() => viewportObserver.observe(scroller))
  onCleanup(() => viewportObserver.disconnect())

  function measureRow(element: HTMLDivElement) {
    observer.observe(element)
  }

  function publishViewport() {
    batch(() => {
      setViewTop(scroller.scrollTop)
      setViewHeight(scroller.clientHeight)
    })
  }

  function snapViewportToBottom() {
    snapVirtualViewport(scroller, (top, height) => {
      batch(() => {
        setViewTop(top)
        setViewHeight(height)
      })
    })
  }

  createEffect(on(selectedSession, () => {
    heights.clear()
    estimates.clear()
    scroller.scrollTop = 0
    batch(() => {
      setMeasured((value) => value + 1)
      setStick(true)
      setAwayFromBottom(false)
      setViewTop(0)
      setViewHeight(scroller.clientHeight)
    })
    // Wait for keyed transcript content and browser layout before snapping the DOM and virtual
    // viewport together. The immediate top reset keeps the interim frame valid rather than blank.
    requestAnimationFrame(snapViewportToBottom)
  }))

  // Untracked stick: content growth follows the bottom, but flipping stick on its own
  // never scrolls, so easing into the stick zone by hand cannot yank the view.
  createEffect(() => {
    const last = entries().at(-1)
    transcriptRevision(last)
    offsets()
    sessionError()
    thinking()
    // A recovery card appearing below the last row must also pull a stuck-to-bottom view down.
    recoverable()
    if (untrack(stick)) {
      queueMicrotask(snapViewportToBottom)
      return
    }
    queueMicrotask(() => {
      const distance = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight
      setAwayFromBottom(shouldShowScrollToBottom(distance))
    })
  })

  // Only user gestures may change stickiness; programmatic snaps, browser clamps, and
  // measurement churn fire scroll events too and used to unstick mid-settle.
  let gestureAt = 0
  let dragging = false
  let scrollLatchReset: ReturnType<typeof setTimeout> | undefined
  const gesture = () => (gestureAt = Date.now())
  const nativeWheel = gesture
  const releaseDrag = () => (dragging = false)
  const forwardedWheel = (event: Event) => {
    const detail = (event as CustomEvent<ForwardedWheel>).detail
    gesture()
    const delta = normalizedWheelDelta(detail.deltaY, detail.deltaMode, scroller.clientHeight)
    const max = Math.max(0, scroller.scrollHeight - scroller.clientHeight)
    scroller.scrollTop = accumulatedWheelTarget(scroller.scrollTop, null, delta, max)
  }
  window.addEventListener("pointerup", releaseDrag)
  window.addEventListener(chatWheelEvent, forwardedWheel)
  onCleanup(() => {
    clearTimeout(scrollLatchReset)
    window.removeEventListener("pointerup", releaseDrag)
    window.removeEventListener(chatWheelEvent, forwardedWheel)
  })

  function onScroll() {
    const top = scroller.scrollTop
    const previous = untrack(viewTop)
    setViewTop(top)
    setViewHeight(scroller.clientHeight)
    if (scroller.classList.contains("transcript-scroll-active") || dragging || Date.now() - gestureAt < gestureWindowMs) {
      scroller.classList.add("transcript-scroll-active")
      clearTimeout(scrollLatchReset)
      scrollLatchReset = setTimeout(() => scroller.classList.remove("transcript-scroll-active"), gestureWindowMs)
    }
    if (dragging || Date.now() - gestureAt < gestureWindowMs) {
      const distance = scroller.scrollHeight - top - scroller.clientHeight
      const nextStick = scrollGestureSticks(previous, top, distance)
      batch(() => {
        setStick(nextStick)
        setAwayFromBottom(!nextStick && shouldShowScrollToBottom(distance))
      })
    }
    maybeLoadOlder(top)
  }

  function maybeLoadOlder(top: number) {
    const id = selectedSession()
    // A stuck view is auto-following the bottom, so any top position it reports is synthetic:
    // the session-switch reset assigns scrollTop = 0 and measurement churn can pass through the
    // top zone before the bottom snap lands. Paging in history for those would fight the snap
    // with competing scroll corrections. Only a user scroll can unstick, so real reads still page.
    if (!id || loadingOlder || untrack(stick) || top > loadOlderAt || !engine.state.cursors[id]) return
    loadingOlder = true
    const before = scroller.scrollHeight - scroller.scrollTop
    void engine.actions.loadOlder(id).finally(() => {
      queueMicrotask(() => {
        scroller.scrollTop = scroller.scrollHeight - before
        loadingOlder = false
      })
    })
  }

  function scrollToBottom() {
    batch(() => {
      setStick(true)
      setAwayFromBottom(false)
    })
    scroller.scrollTo({ top: scroller.scrollHeight, behavior: "smooth" })
  }

  return (
    <div class="relative min-h-0 flex-1">
      <div
        ref={scroller}
        class="transcript-scroll h-full overflow-x-hidden overflow-y-auto"
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
            <Show when={timeline().length === 0 && (revertBackfill() || (!engine.state.loaded[selectedSession()!] && engine.state.connection === "online")) && !sessionError()}>
              <div class="flex justify-center pt-8 text-sm select-none" role="status" aria-live="polite">
                <TextShimmer text={t("common.loading")} />
              </div>
            </Show>
            <div aria-hidden="true" style={{ height: `${offsets()[range().start]}px` }} />
            <For each={slice()}>
              {(entry) => (
                <Row
                  entry={entry}
                  next={nextEntries().get(entry.info.id)}
                  nextThinking={thinkingOnly(nextEntries().get(entry.info.id))}
                  groups={assistantGroups().get(entry.info.id)}
                  thinking={thinking()?.messageID === entry.info.id && !retry()}
                  thinkingCompaction={thinking()?.compaction}
                  thinkingHeading={thinking()?.heading}
                  retry={thinking()?.messageID === entry.info.id ? retry() : undefined}
                  terminalError={!nextEntries().get(entry.info.id) && !!sessionError()}
                  measure={measureRow}
                />
              )}
            </For>
            <div
              aria-hidden="true"
              style={{ height: `${(offsets().at(-1) ?? 0) - offsets()[range().end]}px` }}
            />
            <Show keyed when={recoverable()}>
              {(interruption) => (
                <div class="pb-6">
                  <RecoveryCard interruption={interruption} />
                </div>
              )}
            </Show>
            <Show when={!recoverable() && sessionError()}>
              {(error) => (
                <div role="alert">
                  <div class="rounded-lg border border-danger/40 bg-danger/10 px-3 py-2 text-sm break-words text-danger">
                    {error()}
                  </div>
                </div>
              )}
            </Show>
          </div>
        </Show>
      </div>
      <button
        type="button"
        title="Scroll to latest message"
        aria-label="Scroll to latest message"
        class="group absolute bottom-4 left-1/2 z-10 flex size-9 -translate-x-1/2 items-center justify-center rounded-full border border-edge-strong bg-overlay/95 text-ink-muted shadow-lg shadow-black/25 backdrop-blur transition-[opacity,translate,background-color,border-color,color,scale] duration-200 ease-out hover:border-accent/50 hover:bg-raised hover:text-ink active:scale-95"
        classList={{
          "pointer-events-auto translate-y-0 opacity-100": awayFromBottom(),
          "pointer-events-none translate-y-2 opacity-0": !awayFromBottom(),
        }}
        onClick={scrollToBottom}
      >
        <IconArrowDown class="size-4 transition-transform duration-200 group-hover:translate-y-0.5" />
      </button>
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

export function estimatedTimelineRow(
  entry: MessageEntry,
  fontSize = 13,
  parts: Part[] = entry.parts,
  thinkingOnly = false,
  collapsedSummary = false,
) {
  if (thinkingOnly) return 32
  if (collapsedSummary) return 44
  const text = messageText(parts === entry.parts ? entry : { ...entry, parts })
  const generated = parts.some((part) => part.type === "text" && part.metadata?.generated === true)
  if (entry.info.role === "user" && !generated && largeUserText(text))
    return Math.max(estimatedRow, Math.ceil(text.split("\n").length * fontSize * 1.6 + 62))
  const width = entry.info.role === "user" ? 72 : 88
  const textHeight = estimateTextLines(text, width) * 14 * 1.6
  const toolHeight = parts.filter((part) => part.type === "tool").length * 56
  return Math.max(estimatedRow, Math.ceil(textHeight + toolHeight + (text ? 48 : 0)))
}

export function estimateTextLines(text: string, width: number) {
  let fenced = false
  return text.split("\n").reduce((total, line) => {
    if (/^\s*```/.test(line)) {
      fenced = !fenced
      return total + 1
    }
    if (fenced) return total + 1
    if (!line) return total
    return total + Math.max(1, Math.ceil(line.length / width))
  }, 0)
}

export function resizeCompensation(previous: number, next: number, rowBottom: number, viewportTop: number) {
  return rowBottom < viewportTop ? next - previous : 0
}

export function virtualRange(offsets: number[], viewTop: number, viewHeight: number) {
  const currentTop = Math.min(viewTop, Math.max(0, (offsets.at(-1) ?? 0) - viewHeight))
  const top = currentTop - overscan
  const bottom = currentTop + viewHeight + overscan
  let start = 0
  while (start < offsets.length - 1 && offsets[start + 1] < top) start++
  let end = start
  while (end < offsets.length - 1 && offsets[end] < bottom) end++
  return { start, end }
}

/** Whether an empty reverted timeline still has older pages that could reveal pre-revert rows. */
export function revertBackfillNeeded(input: {
  revertedAt?: string
  visible: number
  loaded?: boolean
  cursor?: string | null
}) {
  return !!input.revertedAt && input.visible === 0 && !!input.loaded && !!input.cursor
}

export function snapVirtualViewport(
  scroller: { scrollTop: number; readonly scrollHeight: number; readonly clientHeight: number },
  publish: (top: number, height: number) => void,
) {
  scroller.scrollTop = scroller.scrollHeight
  // Read back the browser-clamped value and publish it synchronously. A no-op assignment does not
  // have to dispatch a scroll event, which previously left the virtual range at a stale position.
  publish(scroller.scrollTop, scroller.clientHeight)
}

export function scrollGestureSticks(previousTop: number, nextTop: number, distanceFromBottom: number) {
  if (nextTop < previousTop) return false
  return distanceFromBottom < stickyThresholdPx
}

export function shouldShowScrollToBottom(distanceFromBottom: number) {
  return distanceFromBottom >= stickyThresholdPx
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

export function timelineEntries(entries: MessageEntry[], activeMessageID?: string | null) {
  return entries.filter((entry) => entry.info.id === activeMessageID || messageVisible(entry))
}

export function timelinePitch(entry: MessageEntry, next?: MessageEntry) {
  if (!next) return "none" as const
  return assistantFlowContinues(entry, next) ? "part" as const : "turn" as const
}

function timelineParts(entry: MessageEntry, groups?: PartGroup[]) {
  if (entry.info.role !== "assistant" || !groups) return entry.parts
  return groups.flatMap((group) => "explored" in group ? group.explored : [group.part])
}

function timelineRowVisible(entry: MessageEntry, groups: PartGroup[] | undefined, next: MessageEntry | undefined, active?: string) {
  if (entry.info.role === "user") return true
  const info = entry.info as AssistantMessage
  return !!groups?.length || !!info.summary || !!info.error || entry.info.id === active ||
    (!!info.time.completed && next?.info.role !== "assistant")
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
  // A user turn newer than every assistant message has no response row yet, so the indicator
  // anchors under that prompt; otherwise it stays on the assistant turn that is actually running.
  const anchor = unfinished ?? newestFirst.find((entry) => entry.info.role === "user" || entry.info.role === "assistant")
  if (!anchor) return null
  const parentID = anchor.info.role === "user"
    ? anchor.info.id
    : "parentID" in anchor.info ? anchor.info.parentID : undefined
  const assistants = parentID
    ? entries.filter(
      (entry) => entry.info.role === "assistant" && "parentID" in entry.info && entry.info.parentID === parentID,
    )
    : anchor.info.role === "assistant" ? [anchor] : []
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
  const owner = unfinished ?? assistants.at(-1) ?? anchor
  // Compaction turns are the assistant summary message or, in the brief window before it arrives,
  // the user boundary carrying the compaction part. Rows use this to pull the shimmer onto the
  // compaction divider instead of the generic indicator.
  const compaction = owner.info.role === "assistant"
    ? !!(owner.info as { summary?: boolean }).summary
    : owner.parts.some((part) => part.type === "compaction")
  return { messageID: owner.info.id, heading, compaction }
}

// Whether this timeline row renders a compaction divider that can carry the shimmer itself.
// Summary rows only show the divider when the collapsible presentation is enabled; with it off,
// the summary streams as a plain assistant flow and the generic indicator stays.
export function compactionThinkingRow(entry: MessageEntry, collapsible: boolean) {
  if (entry.info.role === "user") return entry.parts.some((part) => part.type === "compaction")
  return collapsible && !!(entry.info as AssistantMessage).summary
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
  nextThinking: boolean
  groups?: PartGroup[]
  thinking: boolean
  thinkingCompaction?: boolean
  thinkingHeading?: string
  retry?: Extract<SessionStatus, { type: "retry" }>
  terminalError: boolean
  measure: (element: HTMLDivElement) => void
}) {
  const fresh = Date.now() - props.entry.info.time.created < freshMessageMs
  // Assistant rows remount during virtualization and session switches; replaying an entrance
  // animation on those makes streamed output flicker, so only fresh user rows fade in.
  const fadeIn = fresh && props.entry.info.role === "user"
  const pitch = () => props.nextThinking ? "none" : props.next ? timelinePitch(props.entry, props.next) : props.terminalError ? "turn" : "none"
  // A running compaction animates its own divider label, so the generic indicator would double up.
  const compactionShimmer = () =>
    props.thinking && !!props.thinkingCompaction && compactionThinkingRow(props.entry, collapseCompaction())
  return (
    <div
      ref={props.measure}
      data-mid={props.entry.info.id}
      class="min-w-0 max-w-full"
      classList={{ "fade-up": fadeIn, "pb-3": pitch() === "part", "pb-6": pitch() === "turn" }}
    >
      <MessageView
        entry={props.entry}
        footer={props.next?.info.role !== "assistant"}
        groups={props.groups}
        thinking={compactionShimmer()}
      />
      <Show when={props.thinking && !compactionShimmer()}>
        <div class="timeline-thinking select-none" role="status" aria-live="polite">
          <TextShimmer text={t("drift.chat.thinking")} />
          <Show when={props.thinkingHeading}>{(heading) => <span class="timeline-thinking-heading">{heading()}</span>}</Show>
        </div>
      </Show>
      <Show when={props.retry}>
        {(status) => (
          <SessionRetry
            status={status()}
            sessionID={props.entry.info.sessionID}
            messageID={props.entry.info.id}
            model={
              props.entry.info.role === "assistant"
                ? { providerID: props.entry.info.providerID, modelID: props.entry.info.modelID }
                : undefined
            }
          />
        )}
      </Show>
    </div>
  )
}

function SessionRetry(props: {
  status: Extract<SessionStatus, { type: "retry" }>
  sessionID: string
  messageID: string
  model?: ModelRef
}) {
  const engine = useEngine()
  const [now, setNow] = createSignal(Date.now())
  // Message updates carrying the pre-switch model would snap a plain mirror of props back to the
  // old selection; a local accepted choice wins until the engine converges on it.
  const [chosen, setChosen] = createSignal<ModelRef>()
  const [submitting, setSubmitting] = createSignal(false)
  const timer = setInterval(() => setNow(Date.now()), 1000)
  onCleanup(() => clearInterval(timer))
  const selected = () => chosen() ?? props.model
  const display = createMemo(() => retryPresentation(props.status, now()))
  const items = createMemo(() => retryModelItems(engine.state))
  const selectedID = () => {
    const model = selected()
    return model ? `${model.providerID}/${model.modelID}` : undefined
  }

  async function switchModel(id: string) {
    if (submitting()) return
    const [providerID, ...rest] = id.split("/")
    const model = { providerID, modelID: rest.join("/") }
    const preferredVariant = prefsFor(props.sessionID).variant
    const variants = Object.keys(modelInfo(engine.state, model)?.variants ?? {})
    const variant = preferredVariant && variants.includes(preferredVariant) ? preferredVariant : undefined
    setSubmitting(true)
    const result = await engine.actions.switchRetryModel(props.sessionID, props.messageID, model, variant)
    setSubmitting(false)
    if (!result.ok) {
      engine.actions.notice({
        message: result.error,
        variant: "error",
      })
      return
    }
    setChosen(model)
    updatePrefs(props.sessionID, { model, variant: variant ?? null })
  }

  return (
    <div class="mt-3 rounded-lg border border-danger/40 bg-danger/10 px-3 py-2 text-sm text-danger" role="status" aria-live="polite">
      <div class="flex flex-wrap items-start justify-between gap-3">
        <div class="flex min-w-0 flex-1 items-start gap-2">
          <span class="pulse-soft mt-1.5 size-2 shrink-0 rounded-full bg-danger" aria-hidden="true" />
          <div class="min-w-0">
            <div class="break-words" classList={{ "cursor-help": display().truncated }} title={display().truncated ? props.status.message : undefined}>
              {display().message}
            </div>
            <div class="mt-0.5 text-xs text-danger/75">{display().info}</div>
          </div>
        </div>
        <Show when={props.model}>
          <Picker
            label={submitting() ? t("drift.recovery.starting") : t("drift.recovery.chooseModel")}
            items={items()}
            selected={selectedID()}
            fallbackLabel={selectedID()}
            icon={<ProviderIcon id={selected()?.providerID} class="size-3.5 shrink-0" />}
            bordered
            floating
            placement="above"
            onPick={(id) => void switchModel(id)}
          />
        </Show>
      </div>
    </div>
  )
}

export function retryPresentation(status: Extract<SessionStatus, { type: "retry" }>, now: number) {
  const normalized = status.message.trim() || t("drift.chat.retry.providerRejected")
  const truncated = normalized.length > maxRetryMessageChars
  const message = truncated ? normalized.slice(0, maxRetryMessageChars) + "..." : normalized
  const seconds = Math.max(0, Math.round((status.next - now) / 1000))
  const retry = seconds > 0 ? t("drift.chat.retry.inSeconds", { seconds }) : t("drift.chat.retry.now")
  return {
    message,
    truncated,
    info: t("drift.chat.retry.info", { retry, attempt: status.attempt }),
  }
}

function EmptyState() {
  return (
    <div class="flex h-full flex-col items-center justify-center gap-3 select-none">
      <DriftLogo class="fade-up size-16 text-ink" label="Drift" />
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
