import { createSignal, onCleanup, Show } from "solid-js"
import { useEngine } from "../engine"
import { contextStats, resolveModel } from "../engine/store"
import { prefsFor } from "../state/prefs"
import { selectedSession, selectSession } from "../state/selection"
import { toggleDebugPanel } from "../state/panels"
import { IconArrowUp } from "./icons"
import { t } from "../state/i18n"

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
                  title={t("drift.thread.backToParent")}
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
                  title={t("drift.thread.copyShareLink", { url: url() })}
                  onClick={() => void navigator.clipboard.writeText(url())}
                >
                  {t("drift.thread.shared")}
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
  return (
    <div class="group/meter relative shrink-0">
      <button
        class="flex h-6 items-center gap-1.5 rounded-md px-1 text-ink-faint transition-colors select-none hover:bg-raised hover:text-ink"
        title={t("context.usage.clickToView")}
        onClick={toggleDebugPanel}
      >
        <svg class="size-4 shrink-0 -rotate-90" viewBox="0 0 20 20" fill="none" aria-hidden="true">
          <circle
            cx="10"
            cy="10"
            r="7"
            pathLength="100"
            stroke="var(--edge-strong)"
            stroke-width="2"
          />
          <circle
            cx="10"
            cy="10"
            r="7"
            pathLength="100"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-dasharray={`${percent()} ${100 - percent()}`}
            class="transition-[stroke-dasharray] duration-300"
          />
        </svg>
        <span class="text-[0.68rem] leading-none font-medium tabular-nums">
          {stats() ? `${percent()}%` : "--"}
        </span>
      </button>
      <div class="context-meter-popover pop-in absolute top-full right-0 z-30 mt-1.5 hidden w-56 rounded-lg border border-edge bg-overlay py-1 shadow-xl shadow-black/40 select-none group-hover/meter:block">
        <Show
          when={stats()}
          fallback={<div class="px-3 py-2 text-xs text-ink-faint">{t("drift.context.pending")}</div>}
        >
          {(usage) => (
            <>
              <MeterRow label={t("context.usage.cost")} value={`$${usage().cost.toFixed(2)}`} />
              <MeterRow label={t("context.usage.usage")} value={`${usage().percent}%`} />
              <MeterRow label={t("context.usage.tokens")} value={usage().count.toLocaleString()} />
              <MeterRow label={t("drift.context.untilCompaction")} value={usage().untilCompaction.toLocaleString()} />
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
          title={t("drift.thread.renameHint")}
          onDblClick={() => setEditing(true)}
        >
          {props.title || t("drift.thread.untitled")}
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
