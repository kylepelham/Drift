import { createSignal, onCleanup, Show } from "solid-js"
import { useEngine } from "../engine"
import { contextStats, resolveModel } from "../engine/store"
import { prefsFor } from "../state/prefs"
import { selectedSession, selectSession } from "../state/selection"
import { toggleDebugPanel } from "../state/panels"
import { IconArrowUp } from "./icons"

const chatColumnWidth = 768

export function ChatHeader() {
  const engine = useEngine()
  const session = () => engine.state.sessions[selectedSession() ?? ""]
  const [transparent, setTransparent] = createSignal(false)
  let row!: HTMLDivElement

  function remeasure() {
    const left = row.firstElementChild?.getBoundingClientRect().width ?? 0
    const right = row.lastElementChild?.getBoundingClientRect().width ?? 0
    const gap = Math.max(0, (row.clientWidth - chatColumnWidth) / 2)
    setTransparent(gap > left + 16 && gap > right + 16)
  }

  function observe(element: HTMLDivElement) {
    row = element
    const observer = new ResizeObserver(remeasure)
    observer.observe(element)
    for (const child of element.children) observer.observe(child)
    onCleanup(() => observer.disconnect())
  }

  const backTarget = () => {
    const current = session()
    if (!current) return undefined
    return current.parentID ?? engine.state.links[current.id]
  }
  return (
    <Show when={session()}>
      {(current) => (
        <div
          ref={observe}
          class="pointer-events-none absolute inset-x-0 top-0 z-10 flex h-11 items-center gap-2 border-b px-4 transition-colors"
          classList={{
            "border-edge bg-bg": !transparent(),
            "border-transparent bg-transparent": transparent(),
          }}
        >
          <div class="pointer-events-auto flex min-w-0 max-w-[45%] items-center gap-2">
            <Show when={backTarget()}>
              {(target) => (
                <button
                  class="flex size-7 shrink-0 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
                  title="Back to the thread this was spawned from"
                  onClick={() => selectSession(target())}
                >
                  <IconArrowUp />
                </button>
              )}
            </Show>
            <Title id={current().id} title={current().title} />
          </div>
          <div class="pointer-events-none min-w-4 flex-1" />
          <div class="pointer-events-auto flex shrink-0 items-center gap-2">
            <ContextMeter sessionId={current().id} />
            <Show when={current().share?.url}>
              {(url) => (
                <button
                  class="shrink-0 rounded-full border border-edge px-2 py-0.5 text-[0.65rem] text-ink-faint transition-colors hover:border-edge-strong hover:text-ink"
                  title={`Copy share link: ${url()}`}
                  onClick={() => void navigator.clipboard.writeText(url())}
                >
                  Shared
                </button>
              )}
            </Show>
          </div>
        </div>
      )}
    </Show>
  )
}

function ContextMeter(props: { sessionId: string }) {
  const engine = useEngine()
  const stats = () => contextStats(engine.state, props.sessionId, resolveModel(engine.state, prefsFor(props.sessionId).model))
  const percent = () => stats()?.percent ?? 0
  const arc = () => percent() * 0.75
  return (
    <div class="group/meter relative shrink-0">
      <button
        class="relative flex size-[2.025rem] items-center justify-center rounded-full text-ink-faint transition-colors select-none hover:text-ink"
        classList={{
          "text-ok": !!stats() && percent() < 60,
          "text-warn": !!stats() && percent() >= 60 && percent() < 85,
          "text-danger": !!stats() && percent() >= 85,
        }}
        title="Open context details"
        onClick={toggleDebugPanel}
      >
        <svg class="size-[2.025rem] -rotate-0" viewBox="0 0 36 36" fill="none" aria-hidden="true">
          <circle
            cx="18"
            cy="18"
            r="13"
            pathLength="100"
            stroke="var(--edge-strong)"
            stroke-width="2.5"
            stroke-linecap="round"
            stroke-dasharray="75 25"
            transform="rotate(135 18 18)"
          />
          <circle
            cx="18"
            cy="18"
            r="13"
            pathLength="100"
            stroke="currentColor"
            stroke-width="2.5"
            stroke-linecap="round"
            stroke-dasharray={`${arc()} ${100 - arc()}`}
            transform="rotate(135 18 18)"
            class="transition-[stroke-dasharray] duration-300"
          />
        </svg>
        <span class="absolute inset-x-0 bottom-0.5 text-center text-[0.6rem] leading-none font-semibold">
          {stats() ? `${percent()}%` : "--"}
        </span>
      </button>
      <div class="pop-in absolute top-full right-0 z-30 mt-1.5 hidden w-56 rounded-lg border border-edge bg-overlay py-1 shadow-xl shadow-black/40 select-none group-hover/meter:block">
        <Show
          when={stats()}
          fallback={<div class="px-3 py-2 text-xs text-ink-faint">Usage appears after the first response.</div>}
        >
          {(usage) => (
            <>
              <MeterRow label="Cost" value={`$${usage().cost.toFixed(2)}`} />
              <MeterRow label="Usage" value={`${usage().percent}%`} />
              <MeterRow label="Tokens" value={usage().count.toLocaleString()} />
              <MeterRow label="Until compaction" value={usage().untilCompaction.toLocaleString()} />
            </>
          )}
        </Show>
      </div>
    </div>
  )
}

function MeterRow(props: { label: string; value: string }) {
  return (
    <div class="flex items-center justify-between px-3 py-1.5 text-xs">
      <span class="text-ink-muted">{props.label}</span>
      <span class="font-semibold text-ink">{props.value}</span>
    </div>
  )
}

function Title(props: { id: string; title: string }) {
  const engine = useEngine()
  const [editing, setEditing] = createSignal(false)

  const commit = (value: string) => {
    const next = value.trim()
    if (next && next !== props.title) void engine.actions.rename(props.id, next)
    setEditing(false)
  }

  return (
    <Show
      when={editing()}
      fallback={
        <span
          class="min-w-0 cursor-text truncate text-sm text-ink"
          title="Double-click to rename"
          onDblClick={() => setEditing(true)}
        >
          {props.title || "Untitled"}
        </span>
      }
    >
      <input
        class="w-64 min-w-0 rounded-md border border-edge bg-surface px-2 py-1 text-sm outline-none focus:border-edge-strong"
        value={props.title}
        ref={(el) => queueMicrotask(() => el.select())}
        onKeyDown={(event) => {
          if (event.key === "Enter") commit(event.currentTarget.value)
          if (event.key === "Escape") setEditing(false)
        }}
        onBlur={(event) => commit(event.currentTarget.value)}
      />
    </Show>
  )
}
