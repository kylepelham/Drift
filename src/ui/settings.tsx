import { createEffect, For, onCleanup, Show } from "solid-js"
import { useEngine } from "../engine"
import { setShowReasoning, showReasoning } from "../state/prefs"
import { setTheme, theme, themes, type ThemeName } from "../state/theme"
import { IconCheck, IconX } from "./icons"
import { Toggle } from "./model-manager"

const themeMeta: Record<ThemeName, { label: string; swatch: [string, string, string] }> = {
  "drift-dark": { label: "Drift Dark", swatch: ["#141517", "#212429", "#7ba3e8"] },
  "drift-slate": { label: "Drift Slate", swatch: ["#0f1419", "#1b232c", "#6cb2c9"] },
  "drift-light": { label: "Drift Light", swatch: ["#f4f4f5", "#ffffff", "#3a6fd8"] },
}

export function SettingsModal(props: { onClose: () => void }) {
  const engine = useEngine()
  const engineVersion = () => engine.state.version
  createEffect(() => {
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") props.onClose()
    }
    document.addEventListener("keydown", escape)
    onCleanup(() => document.removeEventListener("keydown", escape))
  })

  return (
    <div class="fixed inset-0 z-30 flex items-center justify-center bg-black/50" onClick={props.onClose}>
      <div
        class="fade-up w-[26rem] rounded-xl border border-edge bg-overlay shadow-2xl shadow-black/40"
        onClick={(event) => event.stopPropagation()}
      >
        <div class="flex items-center justify-between border-b border-edge px-4 py-3">
          <span class="text-sm font-semibold text-ink">Settings</span>
          <button
            class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
            onClick={props.onClose}
          >
            <IconX />
          </button>
        </div>
        <div class="px-4 py-4">
          <div class="mb-2 text-[0.68rem] tracking-wider text-ink-faint uppercase">Appearance</div>
          <div class="space-y-1">
            <For each={themes}>{(name) => <ThemeRow name={name} />}</For>
          </div>
          <div class="mt-4 mb-2 text-[0.68rem] tracking-wider text-ink-faint uppercase">Chat</div>
          <div
            class="flex cursor-pointer items-center justify-between rounded-lg px-3 py-2.5 hover:bg-raised/60"
            onClick={() => setShowReasoning(!showReasoning())}
          >
            <div>
              <div class="text-sm text-ink">Show thinking</div>
              <div class="text-xs text-ink-faint">Show the model's reasoning above responses.</div>
            </div>
            <Toggle label="Show thinking" checked={showReasoning()} onChange={() => setShowReasoning(!showReasoning())} />
          </div>
          <div class="mt-4 mb-2 text-[0.68rem] tracking-wider text-ink-faint uppercase">About</div>
          <div class="px-3 text-xs text-ink-faint select-text">
            Engine opencode {engineVersion() || "(connecting...)"}
          </div>
        </div>
      </div>
    </div>
  )
}

function ThemeRow(props: { name: ThemeName }) {
  const meta = themeMeta[props.name]
  const active = () => theme() === props.name
  return (
    <button
      class="flex w-full items-center gap-3 rounded-lg border px-3 py-2.5 text-left transition-colors"
      classList={{
        "border-edge-strong bg-raised": active(),
        "border-transparent hover:bg-raised/60": !active(),
      }}
      onClick={() => setTheme(props.name)}
    >
      <span class="flex items-center">
        <For each={meta.swatch}>
          {(color, index) => (
            <span
              class="-ml-1.5 size-4 rounded-full border border-black/30 first:ml-0"
              style={{ background: color, "z-index": 3 - index() }}
            />
          )}
        </For>
      </span>
      <span class="flex-1 text-sm" classList={{ "text-ink": active(), "text-ink-muted": !active() }}>
        {meta.label}
      </span>
      <Show when={active()}>
        <IconCheck class="size-4 text-accent" />
      </Show>
    </button>
  )
}
