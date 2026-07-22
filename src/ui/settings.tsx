import type { ProviderAuthMethod } from "@opencode-ai/sdk/client"
import { createEffect, createMemo, createSignal, For, Match, onCleanup, onMount, Show, Switch } from "solid-js"
import { useEngine } from "../engine"
import { comboFor, eventCombo, formatCombo, keybindDefs, setCombo, type KeybindAction } from "../state/keybinds"
import {
  collapseCompaction,
  compactionCollapsed,
  notifyAttention,
  setCollapseCompaction,
  setCompactionCollapsed,
  setNotifyAttention,
  setShowReasoning,
  showReasoning,
} from "../state/prefs"
import { shellInvoke } from "../state/store"
import { requestNotificationPermission } from "./notifications"
import { setTheme, theme, themes, type ThemeName } from "../state/theme"
import { IconCheck, IconX } from "./icons"
import { Toggle } from "./model-manager"
import { ProviderIcon } from "./provider-icon"

const themeMeta: Record<ThemeName, { label: string; swatch: [string, string, string] }> = {
  "drift-dark": { label: "Drift Dark", swatch: ["#141517", "#212429", "#7ba3e8"] },
  "drift-slate": { label: "Drift Slate", swatch: ["#0f1419", "#1b232c", "#6cb2c9"] },
  "drift-light": { label: "Drift Light", swatch: ["#f4f4f5", "#ffffff", "#3a6fd8"] },
}

const sections = ["General", "Appearance", "Providers", "Keybinds", "About"] as const
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
              <Match when={section() === "Providers"}>
                <ProvidersSection />
              </Match>
              <Match when={section() === "Keybinds"}>
                <KeybindsSection />
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
      <div
        class="flex cursor-pointer items-center justify-between rounded-lg px-3 py-2.5 hover:bg-raised/60"
        onClick={() => setCollapseCompaction(!collapseCompaction())}
      >
        <div>
          <div class="text-sm text-ink">Collapsible compaction summaries</div>
          <div class="text-xs text-ink-faint">
            When a thread runs low on context, the engine compacts it into a summary message. This folds that
            summary behind a divider you can expand.
          </div>
        </div>
        <Toggle
          label="Collapsible compaction summaries"
          checked={collapseCompaction()}
          onChange={() => setCollapseCompaction(!collapseCompaction())}
        />
      </div>
      <div
        class="flex items-center justify-between rounded-lg px-3 py-2.5"
        classList={{
          "cursor-pointer hover:bg-raised/60": collapseCompaction(),
          "opacity-50": !collapseCompaction(),
        }}
        onClick={() => collapseCompaction() && setCompactionCollapsed(!compactionCollapsed())}
      >
        <div>
          <div class="text-sm text-ink">Collapse summaries by default</div>
          <div class="text-xs text-ink-faint">Start compaction summaries folded; expand them per message.</div>
        </div>
        <Toggle
          label="Collapse summaries by default"
          checked={compactionCollapsed()}
          disabled={!collapseCompaction()}
          onChange={() => setCompactionCollapsed(!compactionCollapsed())}
        />
      </div>
    </div>
  )
}

function ProvidersSection() {
  const engine = useEngine()
  const [methods, setMethods] = createSignal<Record<string, ProviderAuthMethod[]>>({})
  const [expanded, setExpanded] = createSignal<string | null>(null)
  const [query, setQuery] = createSignal("")
  onMount(() => void engine.actions.providerAuthMethods().then((map) => setMethods({ ...map })))

  const groups = createMemo(() => {
    const value = query().toLowerCase()
    const matching = engine.state.providers
      .filter((provider) => provider.name.toLowerCase().includes(value) || provider.id.toLowerCase().includes(value))
      .sort((a, b) => a.name.localeCompare(b.name))
    return {
      connected: matching.filter((provider) => engine.state.connected.includes(provider.id)),
      rest: matching.filter((provider) => !engine.state.connected.includes(provider.id)),
    }
  })

  const row = (provider: (typeof engine.state.providers)[number]) => (
    <div class="rounded-lg" classList={{ "bg-raised/40": expanded() === provider.id }}>
      <div
        class="flex cursor-pointer items-center gap-2.5 rounded-lg px-3 py-2 hover:bg-raised/60"
        onClick={() => setExpanded(expanded() === provider.id ? null : provider.id)}
      >
        <ProviderIcon id={provider.id} class="size-4 shrink-0" />
        <span class="min-w-0 flex-1 truncate text-sm text-ink">{provider.name}</span>
        <span
          class="size-1.5 shrink-0 rounded-full"
          classList={{
            "bg-ok": engine.state.connected.includes(provider.id),
            "bg-ink-faint": !engine.state.connected.includes(provider.id),
          }}
        />
      </div>
      <Show when={expanded() === provider.id}>
        <ProviderConnect
          providerId={provider.id}
          methods={methods()[provider.id] ?? [{ type: "api", label: "API key" }]}
          onDone={() => setExpanded(null)}
        />
      </Show>
    </div>
  )

  return (
    <div class="space-y-1">
      <input
        class="mb-2 w-full rounded-lg border border-edge bg-surface px-3 py-2 text-sm outline-none placeholder:text-ink-faint focus:border-edge-strong"
        placeholder="Search providers..."
        value={query()}
        onInput={(event) => setQuery(event.currentTarget.value)}
      />
      <Show when={groups().connected.length > 0}>
        <div class="px-3 pt-1 pb-1 text-[0.68rem] tracking-wider text-ink-faint uppercase">Connected</div>
        <For each={groups().connected}>{row}</For>
      </Show>
      <Show when={groups().rest.length > 0}>
        <div class="px-3 pt-3 pb-1 text-[0.68rem] tracking-wider text-ink-faint uppercase">Not connected</div>
        <For each={groups().rest}>{row}</For>
      </Show>
      <Show when={groups().connected.length === 0 && groups().rest.length === 0}>
        <div class="px-3 py-4 text-sm text-ink-faint">No matching providers.</div>
      </Show>
    </div>
  )
}

function ProviderConnect(props: { providerId: string; methods: ProviderAuthMethod[]; onDone: () => void }) {
  const engine = useEngine()
  const [methodIndex, setMethodIndex] = createSignal(0)
  const [key, setKey] = createSignal("")
  const [code, setCode] = createSignal("")
  const [pending, setPending] = createSignal(false)
  const [error, setError] = createSignal("")
  const [authorization, setAuthorization] = createSignal<{ url: string; method: string; instructions: string } | null>(null)
  const method = () => props.methods[methodIndex()] ?? props.methods[0]

  async function finish(ok: boolean) {
    if (!ok) {
      setError("Connection failed. Check the credentials and try again.")
      setPending(false)
      return
    }
    await engine.actions.refreshProviders()
    setPending(false)
    props.onDone()
  }

  async function connectApi() {
    if (!key().trim()) return
    setPending(true)
    setError("")
    await finish(await engine.actions.setProviderKey(props.providerId, key().trim()))
  }

  async function startOauth() {
    setPending(true)
    setError("")
    const auth = await engine.actions.providerAuthorize(props.providerId, methodIndex()).catch(() => null)
    if (!auth) {
      setError("Could not start the sign-in flow.")
      setPending(false)
      return
    }
    setAuthorization(auth)
    openExternal(auth.url)
    if (auth.method === "auto") {
      await finish(await engine.actions.providerCallback(props.providerId, methodIndex()))
      setAuthorization(null)
      return
    }
    setPending(false)
  }

  async function submitCode() {
    if (!code().trim()) return
    setPending(true)
    setError("")
    await finish(await engine.actions.providerCallback(props.providerId, methodIndex(), code().trim()))
  }

  return (
    <div class="space-y-2 px-3 pt-1 pb-3">
      <Show when={props.methods.length > 1}>
        <div class="flex gap-1.5">
          <For each={props.methods}>
            {(item, index) => (
              <button
                class="rounded-md border px-2 py-0.5 text-xs transition-colors"
                classList={{
                  "border-accent text-ink": index() === methodIndex(),
                  "border-edge text-ink-muted hover:text-ink": index() !== methodIndex(),
                }}
                onClick={() => {
                  setMethodIndex(index())
                  setAuthorization(null)
                  setError("")
                }}
              >
                {item.label}
              </button>
            )}
          </For>
        </div>
      </Show>
      <Show when={method()?.type === "api"}>
        <div class="flex gap-2">
          <input
            type="password"
            class="min-w-0 flex-1 rounded-md border border-edge bg-surface px-2.5 py-1.5 text-sm outline-none focus:border-edge-strong"
            placeholder="API key"
            value={key()}
            onInput={(event) => setKey(event.currentTarget.value)}
            onKeyDown={(event) => event.key === "Enter" && void connectApi()}
          />
          <button
            class="rounded-md bg-accent px-3 py-1 text-xs font-medium text-accent-ink disabled:opacity-40"
            disabled={pending() || !key().trim()}
            onClick={() => void connectApi()}
          >
            {pending() ? "Connecting..." : "Connect"}
          </button>
        </div>
      </Show>
      <Show when={method()?.type === "oauth"}>
        <Show
          when={authorization()}
          fallback={
            <button
              class="rounded-md bg-accent px-3 py-1 text-xs font-medium text-accent-ink disabled:opacity-40"
              disabled={pending()}
              onClick={() => void startOauth()}
            >
              {pending() ? "Waiting for sign-in..." : `Sign in with ${method()?.label ?? "browser"}`}
            </button>
          }
        >
          {(auth) => (
            <div class="space-y-2">
              <div class="text-xs text-ink-muted">{auth().instructions || "Finish signing in via the browser."}</div>
              <div class="text-xs break-all text-ink-faint select-text">{auth().url}</div>
              <Show when={auth().method === "code"}>
                <div class="flex gap-2">
                  <input
                    class="min-w-0 flex-1 rounded-md border border-edge bg-surface px-2.5 py-1.5 text-sm outline-none focus:border-edge-strong"
                    placeholder="Paste the code"
                    value={code()}
                    onInput={(event) => setCode(event.currentTarget.value)}
                    onKeyDown={(event) => event.key === "Enter" && void submitCode()}
                  />
                  <button
                    class="rounded-md bg-accent px-3 py-1 text-xs font-medium text-accent-ink disabled:opacity-40"
                    disabled={pending() || !code().trim()}
                    onClick={() => void submitCode()}
                  >
                    {pending() ? "Verifying..." : "Submit"}
                  </button>
                </div>
              </Show>
              <Show when={auth().method === "auto" && pending()}>
                <div class="pulse-soft text-xs text-ink-faint">Waiting for the browser sign-in to complete...</div>
              </Show>
            </div>
          )}
        </Show>
      </Show>
      <Show when={error()}>
        <div class="text-xs text-danger">{error()}</div>
      </Show>
    </div>
  )
}

function openExternal(url: string) {
  const invoke = shellInvoke()
  if (invoke) {
    void invoke("plugin:opener|open_url", { url }).catch(() => {})
    return
  }
  window.open(url, "_blank")
}

function KeybindsSection() {
  const [capturing, setCapturing] = createSignal<KeybindAction | null>(null)

  createEffect(() => {
    const action = capturing()
    if (!action) return
    const capture = (event: KeyboardEvent) => {
      event.preventDefault()
      event.stopPropagation()
      if (event.key === "Escape") return setCapturing(null)
      const combo = eventCombo(event)
      if (!combo) return
      setCombo(action, combo)
      setCapturing(null)
    }
    document.addEventListener("keydown", capture, true)
    onCleanup(() => document.removeEventListener("keydown", capture, true))
  })

  return (
    <div class="space-y-1">
      <For each={keybindDefs}>
        {(def) => (
          <div class="flex items-center gap-2 rounded-lg px-3 py-2 hover:bg-raised/60">
            <span class="min-w-0 flex-1 truncate text-sm text-ink">{def.label}</span>
            <button
              class="rounded-md border px-2.5 py-1 font-mono text-xs transition-colors"
              classList={{
                "border-accent text-accent": capturing() === def.action,
                "border-edge text-ink-muted hover:border-edge-strong hover:text-ink": capturing() !== def.action,
              }}
              onClick={() => setCapturing(capturing() === def.action ? null : def.action)}
            >
              {capturing() === def.action ? "Press keys..." : formatCombo(comboFor(def.action))}
            </button>
            <button
              title="Unbind"
              class="flex size-6 shrink-0 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
              onClick={() => setCombo(def.action, null)}
            >
              <IconX class="size-3.5" />
            </button>
          </div>
        )}
      </For>
    </div>
  )
}

function AboutSection() {
  const engine = useEngine()
  return (
    <div class="space-y-1 px-3 text-sm text-ink-muted select-text">
      <div>Drift, an embedded-opencode desktop agent.</div>
      <div class="text-xs text-ink-faint">
        Engine opencode {engine.state.version || (engine.state.startupError ? "(failed)" : "(starting...)")}
      </div>
      <Show when={engine.state.startupError}>
        <div class="text-xs text-danger">{engine.state.startupError}</div>
      </Show>
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
