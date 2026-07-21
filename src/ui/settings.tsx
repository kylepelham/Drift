import { createEffect, createSignal, For, Match, onCleanup, Show, Switch } from "solid-js"
import { useEngine } from "../engine"
import { notifyAttention, setNotifyAttention, setShowReasoning, showReasoning } from "../state/prefs"
import { requestNotificationPermission } from "./notifications"
import { setTheme, theme, themes, type ThemeName } from "../state/theme"
import { IconCheck, IconX } from "./icons"
import { Toggle } from "./model-manager"

const themeMeta: Record<ThemeName, { label: string; swatch: [string, string, string] }> = {
  "drift-dark": { label: "Drift Dark", swatch: ["#141517", "#212429", "#7ba3e8"] },
  "drift-slate": { label: "Drift Slate", swatch: ["#0f1419", "#1b232c", "#6cb2c9"] },
  "drift-light": { label: "Drift Light", swatch: ["#f4f4f5", "#ffffff", "#3a6fd8"] },
}

const sections = ["General", "Appearance", "Models", "About"] as const
type Section = (typeof sections)[number]

const [settingsOpen, setSettingsOpen] = createSignal(false)

export function openSettings() {
  setSettingsOpen(true)
}

export function SettingsHost() {
  return (
    <Show when={settingsOpen()}>
      <SettingsModal onClose={() => setSettingsOpen(false)} />
    </Show>
  )
}

function SettingsModal(props: { onClose: () => void }) {
  const [section, setSection] = createSignal<Section>("General")
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
        class="fade-up flex h-[30rem] w-[42rem] overflow-hidden rounded-xl border border-edge bg-overlay shadow-2xl shadow-black/40"
        onClick={(event) => event.stopPropagation()}
      >
        <nav class="flex w-40 shrink-0 flex-col gap-0.5 border-r border-edge p-2">
          <For each={sections}>
            {(name) => (
              <button
                class="rounded-md px-2.5 py-1.5 text-left text-sm transition-colors"
                classList={{
                  "bg-raised text-ink": section() === name,
                  "text-ink-muted hover:bg-raised/60 hover:text-ink": section() !== name,
                }}
                onClick={() => setSection(name)}
              >
                {name}
              </button>
            )}
          </For>
        </nav>
        <div class="flex min-w-0 flex-1 flex-col">
          <div class="flex items-center justify-between border-b border-edge px-4 py-3">
            <span class="text-sm font-semibold text-ink">{section()}</span>
            <button
              class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
              onClick={props.onClose}
            >
              <IconX />
            </button>
          </div>
          <div class="min-h-0 flex-1 overflow-y-auto px-4 py-4">
            <Switch>
              <Match when={section() === "General"}>
                <GeneralSection />
              </Match>
              <Match when={section() === "Appearance"}>
                <div class="space-y-1">
                  <For each={themes}>{(name) => <ThemeRow name={name} />}</For>
                </div>
              </Match>
              <Match when={section() === "Models"}>
                <ModelsSection />
              </Match>
              <Match when={section() === "About"}>
                <AboutSection />
              </Match>
            </Switch>
          </div>
        </div>
      </div>
    </div>
  )
}

function GeneralSection() {
  const toggleNotifications = () => {
    const next = !notifyAttention()
    if (next) requestNotificationPermission()
    setNotifyAttention(next)
  }
  return (
    <div class="space-y-1">
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
      <div
        class="flex cursor-pointer items-center justify-between rounded-lg px-3 py-2.5 hover:bg-raised/60"
        onClick={toggleNotifications}
      >
        <div>
          <div class="text-sm text-ink">Notifications</div>
          <div class="text-xs text-ink-faint">
            Notify when a background thread finishes or asks for permission while Drift is unfocused.
          </div>
        </div>
        <Toggle label="Notifications" checked={notifyAttention()} onChange={toggleNotifications} />
      </div>
    </div>
  )
}

function ModelsSection() {
  const engine = useEngine()
  return (
    <div class="space-y-1">
      <For each={engine.state.providers}>
        {(provider) => (
          <div class="flex items-center gap-2.5 rounded-lg px-3 py-2">
            <span
              class="size-1.5 shrink-0 rounded-full"
              classList={{
                "bg-ok": engine.state.connected.includes(provider.id),
                "bg-ink-faint": !engine.state.connected.includes(provider.id),
              }}
            />
            <span class="min-w-0 flex-1 truncate text-sm text-ink">{provider.name}</span>
            <span class="shrink-0 text-xs text-ink-faint">
              {Object.keys(provider.models).length} models
              {engine.state.connected.includes(provider.id) ? " · connected" : ""}
            </span>
          </div>
        )}
      </For>
      <div class="px-3 pt-3 text-xs text-ink-faint">
        Providers connect through opencode (`opencode auth login`). Model visibility is managed from the model picker.
      </div>
    </div>
  )
}

function AboutSection() {
  const engine = useEngine()
  return (
    <div class="space-y-1 px-3 text-sm text-ink-muted select-text">
      <div>Drift, an embedded-opencode desktop agent.</div>
      <div class="text-xs text-ink-faint">Engine opencode {engine.state.version || "(connecting...)"}</div>
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
