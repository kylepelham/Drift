import { createEffect, createSignal, onCleanup, Show } from "solid-js"

export type ToolTimingState = {
  status: string
  time?: { start?: number; end?: number }
}

const clockSubscribers = new Set<(now: number) => void>()
let clockTimer: ReturnType<typeof setInterval> | undefined

function timestamp(value: unknown) {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined
}

function running(state: ToolTimingState) {
  return state.status === "pending" || state.status === "running"
}

function subscribeClock(subscriber: (now: number) => void) {
  clockSubscribers.add(subscriber)
  subscriber(Date.now())
  clockTimer ??= setInterval(() => {
    const now = Date.now()
    for (const update of clockSubscribers) update(now)
  }, 1000)
  return () => {
    clockSubscribers.delete(subscriber)
    if (clockSubscribers.size || !clockTimer) return
    clearInterval(clockTimer)
    clockTimer = undefined
  }
}

function createSharedNow(active: () => boolean) {
  const [now, setNow] = createSignal(Date.now())
  createEffect(() => {
    if (!active()) return
    const unsubscribe = subscribeClock(setNow)
    onCleanup(unsubscribe)
  })
  return now
}

export function toolElapsedMs(state: ToolTimingState, now: number) {
  const start = timestamp(state.time?.start)
  if (start === undefined) return undefined
  const end = timestamp(state.time?.end)
  if (end === undefined && !running(state)) return undefined
  return Math.max(0, (end ?? now) - start)
}

export function formatToolDuration(ms: number) {
  const totalSeconds = Math.max(0, Math.floor(ms / 1000))
  if (totalSeconds < 60) return `${totalSeconds}s`
  const totalMinutes = Math.floor(totalSeconds / 60)
  if (totalMinutes < 60) return `${totalMinutes}m ${String(totalSeconds % 60).padStart(2, "0")}s`
  return `${Math.floor(totalMinutes / 60)}h ${String(totalMinutes % 60).padStart(2, "0")}m`
}

export function ToolDuration(props: { state: ToolTimingState }) {
  const live = () =>
    running(props.state) && timestamp(props.state.time?.start) !== undefined && timestamp(props.state.time?.end) === undefined
  const now = createSharedNow(live)
  const elapsed = () => toolElapsedMs(props.state, now())
  const duration = () => {
    const value = elapsed()
    return value === undefined ? undefined : formatToolDuration(value)
  }
  return (
    <Show when={duration()} keyed>
      {(value) => <span class="shrink-0 font-mono text-xs text-ink-faint">{value}</span>}
    </Show>
  )
}
