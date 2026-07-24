import { createEffect, createMemo, createSignal, For, Show } from "solid-js"
import { useEngine } from "../engine"
import { contextStats, resolveModel, type MessageEntry } from "../engine/store"
import { prefsFor } from "../state/prefs"
import { debugPanelOpen, setDebugPanelOpen } from "../state/panels"
import { t } from "../state/i18n"
import { selectedSession } from "../state/selection"
import { lightTheme } from "../state/theme"
import { IconX } from "./icons"

export function DebugPanel() {
  const engine = useEngine()
  const entries = () => engine.state.transcripts[selectedSession() ?? ""] ?? []
  const stats = () => {
    const id = selectedSession()
    return id ? contextStats(engine.state, id, resolveModel(engine.state, prefsFor(id).model)) : null
  }
  return (
    <Show when={debugPanelOpen() && selectedSession()}>
      <div class="flex w-[26rem] shrink-0 flex-col border-l border-edge bg-surface">
        <div class="flex items-center justify-between border-b border-edge px-3 py-2.5">
          <span class="text-sm font-semibold text-ink">{t("drift.debug.context")}</span>
          <button
            title={t("common.close")}
            class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
            onClick={() => setDebugPanelOpen(false)}
          >
            <IconX />
          </button>
        </div>
        <Show when={stats()}>
          {(usage) => (
            <div class="space-y-1 border-b border-edge px-3 py-2.5 text-xs text-ink-muted select-text">
              <div class="flex justify-between">
                <span>{t("context.usage.usage")}</span>
                <span class="text-ink">{usage().percent}%</span>
              </div>
              <div class="flex justify-between">
                <span>{t("context.usage.tokens")}</span>
                <span class="text-ink">
                  {usage().count.toLocaleString()} / {usage().context.toLocaleString()}
                </span>
              </div>
              <div class="flex justify-between">
                <span>{t("drift.context.untilCompaction")}</span>
                <span class="text-ink">{usage().untilCompaction.toLocaleString()}</span>
              </div>
              <Show when={usage().cost > 0}>
                <div class="flex justify-between">
                  <span>{t("context.usage.cost")}</span>
                  <span class="text-ink">${usage().cost.toFixed(2)}</span>
                </div>
              </Show>
            </div>
          )}
        </Show>
        <div class="min-h-0 flex-1 overflow-y-auto">
          <For each={entries()}>{(entry) => <DebugRow entry={entry} />}</For>
        </div>
      </div>
    </Show>
  )
}

function DebugRow(props: { entry: MessageEntry }) {
  const [expanded, setExpanded] = createSignal(false)
  const time = () =>
    new Date(props.entry.info.time.created).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })
  return (
    <div class="border-b border-edge/60">
      <button
        class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs transition-colors hover:bg-raised/60"
        onClick={() => setExpanded(!expanded())}
      >
        <span
          class="shrink-0 font-semibold"
          classList={{ "text-accent": props.entry.info.role === "user", "text-ink": props.entry.info.role !== "user" }}
        >
          {props.entry.info.role}
        </span>
        <span class="min-w-0 flex-1 truncate font-mono text-ink-faint">{props.entry.info.id}</span>
        <span class="shrink-0 text-ink-faint">{time()}</span>
      </button>
      <Show when={expanded()}>
        <div class="max-h-96 overflow-auto border-t border-edge/60 bg-bg px-3 py-2 select-text">
          <JsonView value={props.entry} />
        </div>
      </Show>
    </div>
  )
}

function JsonView(props: { value: unknown }) {
  const [html, setHtml] = createSignal("")
  const text = createMemo(() => JSON.stringify(props.value, null, 2))
  let generation = 0
  createEffect(() => {
    const value = text()
    const shikiTheme = lightTheme() ? "github-light" : "github-dark-default"
    const current = ++generation
    if (value.length > 200_000) return setHtml("")
    void import("shiki").then(async (shiki) => {
      const output = await shiki.codeToHtml(value, { lang: "json", theme: shikiTheme }).catch(() => "")
      if (current === generation) setHtml(output)
    })
  })
  return (
    <Show
      when={html()}
      fallback={
        <pre class="font-mono text-[0.7rem] leading-relaxed whitespace-pre-wrap text-ink-muted">{text()}</pre>
      }
    >
      <div
        class="font-mono text-[0.7rem] leading-relaxed [&_pre]:!bg-transparent [&_pre]:whitespace-pre-wrap"
        innerHTML={html()}
      />
    </Show>
  )
}
