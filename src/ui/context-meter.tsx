import { createEffect, createMemo, createSignal, For, on, Show } from "solid-js"
import { useEngine } from "../engine"
import { estimateContextBreakdown, type BreakdownKey, type BreakdownSegment } from "../engine/context-breakdown"
import { contextStats, resolveModel, savedChoice } from "../engine/store"
import { t } from "../state/i18n"
import { toggleDebugPanel } from "../state/panels"
import { ProviderIcon } from "./provider-icon"
import { prefsFor } from "../state/prefs"
import {
  planLabel,
  refreshUsage,
  resetLabel,
  resetTitle,
  usageFor,
  usageTone,
  windowLabel,
  type UsageEntry,
  type UsageTone,
  type UsageWindow,
} from "../state/usage-limits"

const segmentColor: Record<BreakdownKey, string> = {
  system: "var(--accent)",
  user: "var(--ok)",
  assistant: "var(--ink-muted)",
  tool: "var(--warn)",
}
const segmentLabel: Record<BreakdownKey, string> = {
  system: "drift.context.systemAndTools",
  user: "drift.context.user",
  assistant: "drift.context.assistant",
  tool: "drift.context.tool",
}
const toneColor: Record<UsageTone, string> = { normal: "var(--accent)", warn: "var(--warn)", danger: "var(--danger)" }
const toneText: Record<UsageTone, string> = { normal: "text-ink-faint", warn: "text-warn", danger: "text-danger" }
const compact = new Intl.NumberFormat(undefined, { notation: "compact", maximumFractionDigits: 1 })

export function ContextMeter(props: { sessionId: string }) {
  const engine = useEngine()
  const model = () => resolveModel(engine.state, prefsFor(props.sessionId, savedChoice(engine.state, props.sessionId)).model)
  const stats = () => contextStats(engine.state, props.sessionId, model())
  const percent = () => stats()?.percent ?? 0
  const refresh = () => {
    const provider = model()?.providerID
    if (provider) void refreshUsage(provider)
  }
  createEffect(on(() => engine.state.status[props.sessionId]?.type, (type) => type === "idle" && refresh()))
  return (
    <div class="group/meter relative shrink-0" onMouseEnter={refresh}>
      <button
        class={`flex h-6 items-center gap-1.5 rounded-md px-1 transition-colors select-none hover:bg-raised hover:text-ink ${toneText[usageTone(percent())]}`}
        title={t("context.usage.clickToView")}
        onClick={toggleDebugPanel}
      >
        <MeterRing percent={percent()} />
        <span class="text-[0.68rem] leading-none font-medium tabular-nums">{stats() ? `${percent()}%` : "--"}</span>
      </button>
      <div class="context-meter-popover absolute top-full right-0 z-30 hidden pt-1.5 group-hover/meter:block">
        <div class="pop-in w-80 rounded-lg border border-edge bg-overlay py-1 shadow-xl shadow-black/40 select-none">
          <ContextSection sessionId={props.sessionId} />
          <Show when={model()?.providerID}>
            {(provider) => <UsageSection provider={provider()} />}
          </Show>
        </div>
      </div>
    </div>
  )
}

function MeterRing(props: { percent: number }) {
  return (
    <svg class="size-4 shrink-0 -rotate-90" viewBox="0 0 20 20" fill="none" aria-hidden="true">
      <circle cx="10" cy="10" r="7" pathLength="100" stroke="var(--edge-strong)" stroke-width="2" />
      <circle
        cx="10"
        cy="10"
        r="7"
        pathLength="100"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-dasharray={`${props.percent} ${100 - props.percent}`}
        class="transition-[stroke-dasharray] duration-300"
      />
    </svg>
  )
}

export function ContextSection(props: { sessionId: string }) {
  const engine = useEngine()
  const stats = () => contextStats(engine.state, props.sessionId, resolveModel(engine.state, prefsFor(props.sessionId, savedChoice(engine.state, props.sessionId)).model))
  const segments = createMemo(() => {
    const usage = stats()
    return usage ? estimateContextBreakdown(engine.state.transcripts[props.sessionId] ?? [], usage.count) : []
  })
  return (
    <Show when={stats()} fallback={<div class="px-3 py-2 text-xs text-ink-faint">{t("drift.context.pending")}</div>}>
      {(usage) => (
        <div class="space-y-2 px-3 py-2 text-xs">
          <div class="flex items-center justify-between gap-2">
            <span class="text-ink-muted">{t("drift.context.window")}</span>
            <span class="text-ink tabular-nums">
              {compact.format(usage().count)} / {compact.format(usage().context)} ({usage().percent}%)
            </span>
          </div>
          <BreakdownBar segments={segments()} context={usage().context} />
          <div class="flex justify-between text-ink-muted">
            <span>{t("drift.context.untilCompaction")}</span>
            <span class="text-ink tabular-nums">{usage().untilCompaction.toLocaleString()}</span>
          </div>
          <Show when={usage().cost > 0}>
            <div class="flex justify-between text-ink-muted">
              <span>{t("context.usage.cost")}</span>
              <span class="text-ink tabular-nums">${usage().cost.toFixed(2)}</span>
            </div>
          </Show>
        </div>
      )}
    </Show>
  )
}

function BreakdownBar(props: { segments: BreakdownSegment[]; context: number }) {
  const [hovered, setHovered] = createSignal<number>()
  const share = (tokens: number) => (tokens / props.context) * 100
  const offset = (index: number) => props.segments.slice(0, index).reduce((sum, segment) => sum + share(segment.tokens), 0)
  const tip = () => {
    const index = hovered()
    const segment = index === undefined ? undefined : props.segments[index]
    if (index === undefined || !segment) return undefined
    return { segment, center: Math.min(85, Math.max(15, offset(index) + share(segment.tokens) / 2)) }
  }
  return (
    <div class="relative" onMouseLeave={() => setHovered(undefined)}>
      <Show when={tip()}>
        {(current) => (
          <div
            class="pointer-events-none absolute bottom-full z-10 mb-1 -translate-x-1/2 rounded-md border border-edge bg-raised px-2 py-1 text-[0.68rem] whitespace-nowrap text-ink shadow-lg shadow-black/30"
            style={{ left: `${current().center}%` }}
          >
            {t(segmentLabel[current().segment.key])}
            <span class="ml-1.5 text-ink-faint tabular-nums">
              {compact.format(current().segment.tokens)} · {Math.round(share(current().segment.tokens))}%
            </span>
          </div>
        )}
      </Show>
      <div class="flex h-3 w-full items-center" data-context-bar>
        <div class="flex h-1.5 w-full overflow-hidden rounded-full bg-edge-strong">
          <For each={props.segments}>
            {(segment, index) => (
              <div
                class="h-full transition-opacity"
                classList={{ "opacity-50": hovered() !== undefined && hovered() !== index() }}
                style={{ width: `${share(segment.tokens)}%`, "background-color": segmentColor[segment.key] }}
                onMouseEnter={() => setHovered(index())}
              />
            )}
          </For>
        </div>
      </div>
    </div>
  )
}

export function usageMessage(entry: UsageEntry | undefined) {
  if (!entry || (entry.loading && !entry.usage)) return t("drift.usage.loading")
  if (entry.usage?.status === "expired") return t("drift.usage.expired")
  if (entry.usage?.status === "unsubscribed") return t("drift.usage.unsubscribed")
  if (entry.failed && !entry.usage) return t("drift.usage.failed")
  if (!entry.usage?.windows.length) return t("drift.usage.empty")
  return ""
}

export function UsageSection(props: { provider: string }) {
  const engine = useEngine()
  const entry = () => usageFor(props.provider)
  const providerName = () => engine.state.providers.find((provider) => provider.id === props.provider)?.name ?? props.provider
  const plan = () => entry()?.usage?.plan
  return (
    <Show when={entry()?.usage !== null}>
      <div class="space-y-2 border-t border-edge px-3 py-2 text-xs" data-usage-limits>
        <div class="flex items-center justify-between gap-2 text-ink-muted">
          <span>
            {t("drift.usage.title")}
            <Show when={plan()}>{(name) => ` · ${planLabel(name())}`}</Show>
          </span>
          <span class="flex min-w-0 items-center gap-1.5 text-ink-faint">
            <ProviderIcon id={props.provider} class="size-3.5 shrink-0" />
            <span class="truncate">{providerName()}</span>
          </span>
        </div>
        <Show when={usageMessage(entry())} fallback={<For each={entry()?.usage?.windows}>{(window) => <LimitRow window={window} />}</For>}>
          {(message) => <div class="text-ink-faint">{message()}</div>}
        </Show>
      </div>
    </Show>
  )
}

export function LimitRow(props: { window: UsageWindow }) {
  const percent = () => Math.round(props.window.usedPercent)
  const tone = () => usageTone(props.window.usedPercent)
  return (
    <div class="space-y-1">
      <div class="flex items-center justify-between gap-2">
        <span class="font-medium text-ink">{windowLabel(props.window)}</span>
        <span class="flex shrink-0 items-center gap-2 text-ink-faint">
          <span title={resetTitle(props.window.resetsAt)}>{resetLabel(props.window.resetsAt)}</span>
          <span class="text-ink tabular-nums">{percent()}%</span>
        </span>
      </div>
      <div class="h-1.5 w-full overflow-hidden rounded-full bg-edge-strong">
        <div
          class="h-full rounded-full transition-[width] duration-300"
          style={{ width: `${props.window.usedPercent}%`, "background-color": toneColor[tone()] }}
        />
      </div>
    </div>
  )
}
