import { createEffect, createMemo, createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js"
import { useEngine } from "../../engine"
import type { McpServerConfig, McpServerStatus } from "../../engine/store"
import { registryConfig, registryServerName, type RegistryServer } from "../../mcp-registry"
import { createRegistrySearch } from "../../state/mcp-registry-search"
import { t } from "../../state/i18n"
import { Toggle } from "../controls"
import { IconCheck, IconPlug, IconPlugOff, IconPlus, IconSquarePen, IconTrash } from "../icons"
import { McpEditor } from "./editor"

type RuntimeAction = "connect" | "disconnect"
type RowKey = "ArrowUp" | "ArrowDown" | "Home" | "End"
type EditorEntry = { server?: McpServerStatus }

/** Connect and disconnect apply only to an enabled server. */
export function mcpRuntimeAction(server: McpServerStatus): RuntimeAction | undefined {
  if (server.state === "connected") return "disconnect"
  if (server.state === "disconnected" || server.state === "failed") return "connect"
}

export function mcpRuntimeKeyAction(server: McpServerStatus, key: string): RuntimeAction | undefined {
  const action = mcpRuntimeAction(server)
  if (key === "ArrowLeft") return action === "disconnect" ? action : undefined
  if (key === "ArrowRight") return action === "connect" ? action : undefined
  if (key === "Enter") return action
}

export function nextMcpRowName(names: string[], current: string, key: RowKey) {
  if (!names.length) return ""
  if (key === "Home") return names[0]
  if (key === "End") return names.at(-1)!
  const index = Math.max(0, names.indexOf(current))
  if (key === "ArrowUp") return names[(index - 1 + names.length) % names.length]
  return names[(index + 1) % names.length]
}

export function McpManagement(props: { embedded?: boolean }) {
  const engine = useEngine()
  const [editor, setEditor] = createSignal<EditorEntry | null>(null)
  const [view, setView] = createSignal<"servers" | "registry">("servers")
  const [confirmRemove, setConfirmRemove] = createSignal("")
  const [message, setMessage] = createSignal("")
  const [failure, setFailure] = createSignal("")
  const [busy, setBusy] = createSignal<string | null>(null)
  const [loading, setLoading] = createSignal(false)
  const [selected, setSelected] = createSignal("")
  const rowElements = new Map<string, HTMLDivElement>()
  const offline = () => engine.state.connection !== "online"
  const locked = () => offline() || !!busy()
  const rowNames = createMemo(() => Object.keys(engine.state.mcpServers).sort((a, b) => a.localeCompare(b)))
  const moveRow = (key: RowKey, current = selected()) => {
    const next = nextMcpRowName(rowNames(), current, key)
    if (!next) return
    setSelected(next)
    rowElements.get(next)?.focus()
  }
  createEffect(() => {
    const names = new Set(rowNames())
    for (const name of rowElements.keys()) if (!names.has(name)) rowElements.delete(name)
    if (!names.has(selected())) setSelected(rowNames()[0] ?? "")
  })
  const refresh = async () => {
    setLoading(true)
    setFailure("")
    try {
      await engine.actions.refreshMcp()
    } catch (error) {
      setFailure(errorText(error))
    } finally {
      setLoading(false)
    }
  }
  onMount(() => void refresh())
  /** One change at a time, owned by the row it acts on, so only that row shows it working. */
  const run = async (name: string, action: () => Promise<unknown>, success?: string) => {
    if (busy()) return false
    setBusy(name)
    setMessage("")
    setFailure("")
    try {
      await action()
      if (success) setMessage(success)
      return true
    } catch (error) {
      setFailure(errorText(error))
      return false
    } finally {
      setBusy(null)
    }
  }
  const save = async (name: string, config: McpServerConfig) => {
    const previous = editor()?.server?.name
    setMessage("")
    setFailure("")
    setBusy(name)
    try {
      // Renamed first, so the save that follows keeps its saved secrets; a retry after a failed save saves under the new name.
      if (previous && previous !== name) setEditor({ server: await engine.actions.mcpRename(previous, name) })
      await engine.actions.mcpSave(name, config, { create: !previous })
      setEditor(null)
    } finally {
      setBusy(null)
    }
  }
  const remove = async (name: string) => {
    if (confirmRemove() !== name) return setConfirmRemove(name)
    if (await run(name, () => engine.actions.mcpRemove(name), t("drift.mcp.removed", { name }))) setConfirmRemove("")
  }
  const runtime = (server: McpServerStatus, action: RuntimeAction) =>
    void run(server.name, () => (action === "connect" ? engine.actions.mcpConnect(server.name) : engine.actions.mcpDisconnect(server.name)))

  return (
    <div class="space-y-3">
      <div class="flex items-center justify-between gap-3">
        <div class="flex rounded-lg border border-edge bg-surface p-0.5">
          <Tab
            active={view() === "servers"}
            autofocus={!props.embedded}
            onClick={() => setView("servers")}
            onKeyDown={(event) => {
              if (["ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) {
                event.preventDefault()
                moveRow(event.key as RowKey)
              }
            }}
          >
            {t("drift.mcp.servers")}
          </Tab>
          <Tab active={view() === "registry"} onClick={() => setView("registry")}>
            {t("drift.mcp.registry")}
          </Tab>
        </div>
        <Show when={view() === "servers"}>
          <button
            class="flex items-center gap-1.5 rounded-md bg-accent px-2.5 py-1.5 text-xs font-medium text-accent-ink disabled:opacity-40"
            disabled={locked()}
            onClick={() => setEditor({})}
          >
            <IconPlus class="size-3.5" />
            {t("drift.mcp.add")}
          </button>
        </Show>
      </div>
      <Show when={failure() || message()}>
        <div
          role={failure() ? "alert" : "status"}
          class="rounded-md border px-3 py-2 text-xs"
          classList={{
            "border-danger/35 bg-danger/10 text-danger": !!failure(),
            "border-ok/35 bg-ok/10 text-ok": !failure(),
          }}
        >
          {failure() || message()}
          <Show when={failure()}>
            <button
              class="ml-3 rounded border border-current px-2 py-1 disabled:opacity-40"
              disabled={loading() || !!busy()}
              onClick={() => void refresh()}
            >
              {t("drift.mcp.retry")}
            </button>
          </Show>
        </div>
      </Show>
      <Show when={view() === "servers"}>
        <Show when={loading()}>
          <div role="status" class="flex items-center gap-2 px-3 py-2 text-sm text-ink-muted">
            <span aria-hidden="true" class="size-3.5 shrink-0 rounded-full border-2 border-ink-faint/30 border-t-ink-muted motion-safe:animate-spin" />
            {t(rowNames().length ? "drift.mcp.refreshing" : "drift.mcp.loading")}
          </div>
        </Show>
        <div aria-busy={loading()} classList={{ "space-y-1": !props.embedded, "border-y border-edge/80": props.embedded }}>
          <For each={rowNames()}>
            {(name) => (
              <Show when={engine.state.mcpServers[name]}>
                {(server) => (
                  <ServerRow
                    server={server()}
                    selected={selected() === name}
                    embedded={props.embedded}
                    disabled={locked()}
                    busy={busy() === name}
                    confirming={confirmRemove() === name}
                    rowRef={(element) => rowElements.set(name, element)}
                    onFocus={() => setSelected(name)}
                    onNavigate={(key) => moveRow(key, name)}
                    onEdit={() => setEditor({ server: server() })}
                    onRemove={() => void remove(name)}
                    onEnabled={(enabled) => void run(name, () => engine.actions.mcpSetEnabled(name, enabled))}
                    onRuntime={(action) => runtime(server(), action)}
                  />
                )}
              </Show>
            )}
          </For>
          <Show when={!loading() && !failure() && !rowNames().length}>
            <div class="px-3 py-5 text-sm text-ink-faint">{t("dialog.mcp.empty")}</div>
          </Show>
        </div>
      </Show>
      <Show when={view() === "registry"}>
        <McpRegistry
          embedded={props.embedded}
          disabled={locked()}
          installed={new Set(rowNames())}
          onInstall={(server) => {
            const config = registryConfig(server)
            if (!config) return setMessage(t("drift.mcp.registryUnavailable"))
            const name = registryServerName(server.name)
            void run(name, () => engine.actions.mcpSave(name, config, { create: true }), t("drift.mcp.installed", { name: server.title ?? server.name }))
          }}
        />
      </Show>
      <Show when={editor()}>
        {(entry) => (
          <McpEditor
            server={entry().server ? { name: entry().server!.name, config: entry().server!.config } : undefined}
            pending={!!busy()}
            onClose={() => setEditor(null)}
            onSave={save}
          />
        )}
      </Show>
    </div>
  )
}

function ServerRow(props: {
  server: McpServerStatus
  selected: boolean
  embedded?: boolean
  disabled: boolean
  busy: boolean
  confirming: boolean
  rowRef: (element: HTMLDivElement) => void
  onFocus: () => void
  onNavigate: (key: RowKey) => void
  onEdit: () => void
  onRemove: () => void
  onEnabled: (enabled: boolean) => void
  onRuntime: (action: RuntimeAction) => void
}) {
  const status = () => statusLabel(props.server, props.busy)
  const runtime = () => mcpRuntimeAction(props.server)
  return (
    <div
      ref={props.rowRef}
      data-mcp-row={props.server.name}
      tabIndex={props.selected ? 0 : -1}
      aria-label={props.server.name}
      class="px-3 py-2.5 outline-none hover:bg-raised/40 focus-visible:bg-raised/50"
      classList={{
        "rounded-lg border border-transparent hover:border-edge": !props.embedded,
        "border-b border-edge/70 last:border-b-0": props.embedded,
        "border-edge-strong bg-raised/30": props.selected && !props.embedded,
      }}
      onFocus={(event) => {
        if (event.target === event.currentTarget) props.onFocus()
      }}
      onClick={(event) => event.currentTarget.focus()}
      onKeyDown={(event) => {
        if (event.target !== event.currentTarget) return
        if (["ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) {
          event.preventDefault()
          props.onNavigate(event.key as RowKey)
          return
        }
        const action = props.disabled ? undefined : mcpRuntimeKeyAction(props.server, event.key)
        if (!action) return
        event.preventDefault()
        props.onRuntime(action)
      }}
    >
      <div class="flex items-start gap-3">
        <div class="min-w-0 flex-1">
          <div class="truncate text-sm font-medium text-ink">{props.server.name}</div>
          <div class="mt-0.5 flex flex-wrap items-center gap-x-2 text-xs">
            <span class={status().tone}>{status().text}</span>
          </div>
        </div>
        <div class="flex shrink-0 flex-wrap items-center justify-end gap-1.5">
          <Action
            disabled={props.disabled}
            tone="danger"
            title={props.confirming ? t("drift.mcp.confirmRemove") : t("drift.mcp.remove")}
            onClick={props.onRemove}
          >
            {/* The confirm step keeps its text: an icon cannot ask "are you sure". */}
            {props.confirming ? t("drift.mcp.confirmRemove") : <IconTrash class="size-3.5" />}
          </Action>
          <Action disabled={props.disabled} title={t("common.edit")} onClick={props.onEdit}>
            <IconSquarePen class="size-3.5" />
          </Action>
          <Show when={runtime()}>
            {(action) => (
              <Action disabled={props.disabled} title={t(`common.${action()}`)} onClick={() => props.onRuntime(action())}>
                {action() === "disconnect" ? <IconPlugOff class="size-3.5" /> : <IconPlug class="size-3.5" />}
              </Action>
            )}
          </Show>
          <Toggle
            label={t("drift.mcp.enable", { name: props.server.name })}
            checked={props.server.enabled}
            disabled={props.disabled}
            onChange={() => props.onEnabled(!props.server.enabled)}
          />
        </div>
      </div>
    </div>
  )
}

function statusLabel(server: McpServerStatus, busy: boolean) {
  if (busy) return { text: t("common.loading"), tone: "text-ink-faint" }
  switch (server.state) {
    case "connected":
      return { text: t("mcp.status.connected"), tone: "text-ok" }
    case "connecting":
      return { text: t("drift.mcp.status.connecting"), tone: "text-ink-faint" }
    case "disconnected":
      return { text: t("drift.mcp.status.disconnected"), tone: "text-ink-faint" }
    case "failed":
      return { text: server.error || t("mcp.status.failed"), tone: "text-danger" }
    case "disabled":
      return { text: t("mcp.status.disabled"), tone: "text-ink-faint" }
  }
}

function errorText(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

function McpRegistry(props: {
  installed: Set<string>
  disabled: boolean
  embedded?: boolean
  onInstall: (server: RegistryServer) => void
}) {
  const [query, setQuery] = createSignal("")
  const [servers, setServers] = createSignal<RegistryServer[]>([])
  const [loading, setLoading] = createSignal(false)
  const [error, setError] = createSignal("")
  const registry = createRegistrySearch()
  let request = 0
  let disposed = false
  const search = async () => {
    const current = ++request
    setLoading(true)
    setError("")
    try {
      const result = await registry.search(query())
      if (!disposed && current === request && !result.stale)
        setServers(result.servers.filter((server) => registryConfig(server)))
    } catch {
      if (!disposed && current === request) setError(t("drift.mcp.registryLoadFailed"))
    } finally {
      if (!disposed && current === request) setLoading(false)
    }
  }
  onMount(() => void search())
  let timer: number | undefined
  onCleanup(() => {
    disposed = true
    request++
    window.clearTimeout(timer)
    registry.dispose()
  })
  const schedule = (value: string) => {
    setQuery(value)
    window.clearTimeout(timer)
    timer = window.setTimeout(() => void search(), 250)
  }
  const installed = (server: RegistryServer) => props.installed.has(registryServerName(server.name))
  return (
    <div class="space-y-2">
      <TextInput value={query()} onInput={schedule} label={t("drift.mcp.registrySearch")} />
      <div class="text-[0.7rem] text-ink-faint">{t("drift.mcp.registrySource")}</div>
      <Show when={error()}>{(value) => <div class="text-xs text-danger">{value()}</div>}</Show>
      <div classList={{ "space-y-2": !props.embedded, "border-y border-edge/80": props.embedded }}>
        <For each={servers()}>
          {(server) => (
            <div
              class="px-3 py-2.5"
              classList={{
                "rounded-lg border border-edge bg-surface": !props.embedded,
                "border-b border-edge/70": props.embedded,
              }}
            >
              <div class="flex items-start gap-3">
                <div class="min-w-0 flex-1">
                  <div class="truncate text-sm font-medium text-ink">{server.title ?? server.name}</div>
                  <div class="text-[0.7rem] text-ink-faint">
                    {server.name} · {server.version}
                  </div>
                  <div class="mt-1 text-xs text-ink-muted">{server.description}</div>
                </div>
                <Action disabled={props.disabled || installed(server)} onClick={() => props.onInstall(server)}>
                  {installed(server) ? <IconCheck class="size-3.5" /> : <IconPlus class="size-3.5" />}
                  {t(installed(server) ? "drift.mcp.installedLabel" : "drift.mcp.install")}
                </Action>
              </div>
            </div>
          )}
        </For>
        <Show when={loading()}>
          <div class="px-3 py-4 text-sm text-ink-faint">{t("common.loading")}</div>
        </Show>
        <Show when={!loading() && !error() && !servers().length}>
          <div class="px-3 py-4 text-sm text-ink-faint">{t("palette.empty")}</div>
        </Show>
      </div>
    </div>
  )
}

function Tab(props: {
  active: boolean
  autofocus?: boolean
  onClick: () => void
  onKeyDown?: JSX.EventHandler<HTMLButtonElement, KeyboardEvent>
  children: JSX.Element
}) {
  return (
    <button
      type="button"
      autofocus={props.autofocus}
      aria-pressed={props.active}
      class="min-w-0 flex-1 rounded-md px-2.5 py-1.5 text-xs"
      classList={{ "bg-raised text-ink": props.active, "text-ink-faint hover:text-ink": !props.active }}
      onClick={props.onClick}
      onKeyDown={props.onKeyDown}
    >
      {props.children}
    </button>
  )
}

function TextInput(props: { value: string; onInput: (value: string) => void; label: string }) {
  return (
    <input
      aria-label={props.label}
      class="h-9 w-full rounded-md border border-edge bg-raised/45 px-2.5 text-sm text-ink outline-none transition-colors placeholder:text-ink-faint focus:border-accent"
      placeholder={props.label}
      value={props.value}
      onInput={(event) => props.onInput(event.currentTarget.value)}
    />
  )
}

function Action(props: {
  disabled?: boolean
  tone?: "danger"
  title?: string
  onClick: () => void
  children: JSX.Element
}) {
  return (
    <button
      type="button"
      disabled={props.disabled}
      title={props.title}
      aria-label={props.title}
      class="flex items-center gap-1 rounded-md border px-2 py-1 text-xs disabled:opacity-40"
      classList={{
        "border-edge text-ink-muted hover:text-ink": !props.tone,
        "border-danger/40 text-danger hover:bg-danger/10": props.tone === "danger",
      }}
      onClick={(event) => {
        event.stopPropagation()
        props.onClick()
      }}
    >
      {props.children}
    </button>
  )
}
