import { createEffect, createMemo, createSignal, For, onMount, Show, type JSX } from "solid-js"
import { useEngine } from "../../engine"
import type { McpServerConfig, McpServerStatus } from "../../engine/store"
import { registryInstallName, type RegistryServer } from "../../mcp-registry"
import { createRegistrySearch } from "../../state/mcp-registry-search"
import { t } from "../../state/i18n"
import { activeWorkspace } from "../../state/workspaces"
import { openExternal } from "../../shell"
import { Toggle } from "../controls"
import { IconPlug, IconPlugOff, IconPlus, IconSquarePen, IconTrash } from "../icons"
import { forgetMcpLogo, LogoTile, mcpLogos, rememberMcpLogo } from "../logo-tile"
import { McpEditor } from "./editor"
import { McpRegistry } from "./registry"

type RuntimeAction = "connect" | "disconnect"
type RowKey = "ArrowUp" | "ArrowDown" | "Home" | "End"
type EditorEntry = { server?: McpServerStatus }

/** Whether turns in workspace `workspaceId` are offered the server: that workspace's own choice, else the server's switch. */
export function mcpOnIn(server: Pick<McpServerStatus, "enabled" | "workspaces">, workspaceId?: string) {
  const chosen = workspaceId === undefined ? undefined : server.workspaces.find((choice) => choice.workspaceId === workspaceId)
  return chosen ? chosen.enabled : server.enabled
}

/** Connect and disconnect apply to a server this build can read; in a workspace that has it off, connect turns it on there. */
export function mcpRuntimeAction(server: McpServerStatus, workspaceId?: string): RuntimeAction | undefined {
  if (server.unreadable) return undefined
  if (workspaceId !== undefined && !mcpOnIn(server, workspaceId)) return "connect"
  if (server.state === "connected") return "disconnect"
  if (server.state === "disconnected" || server.state === "failed") return "connect"
}

export function mcpRuntimeKeyAction(server: McpServerStatus, key: string, workspaceId?: string): RuntimeAction | undefined {
  const action = mcpRuntimeAction(server, workspaceId)
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
  const here = () => activeWorkspace()?.path
  const hereId = () => activeWorkspace()?.id
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
  onMount(() => void refresh().then(fillLogos))
  /** Servers installed before logos were remembered take theirs from the registry catalog, when it has them. */
  const fillLogos = async () => {
    const missing = rowNames().filter((name) => !mcpLogos()[name])
    if (!missing.length) return
    const registry = createRegistrySearch()
    try {
      const result = await registry.search("")
      for (const server of result.servers) {
        const name = registryInstallName(server)
        if (missing.includes(name)) rememberMcpLogo(name, server.listing?.image)
      }
    } catch {
      return
    } finally {
      registry.dispose()
    }
  }
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
  const save = async (name: string, config: McpServerConfig, readOnlyTrusted: boolean) => {
    const previous = editor()?.server?.name
    setMessage("")
    setFailure("")
    setBusy(name)
    try {
      // Renamed first, so the save that follows keeps its saved secrets; a retry after a failed save saves under the new name.
      if (previous && previous !== name) setEditor({ server: await engine.actions.mcpRename(previous, name, here()) })
      await engine.actions.mcpSave(name, config, { create: !previous, readOnlyTrusted, directory: here() })
      setEditor(null)
    } finally {
      setBusy(null)
    }
  }
  const remove = async (name: string) => {
    if (confirmRemove() !== name) return setConfirmRemove(name)
    if (await run(name, () => engine.actions.mcpRemove(name), t("drift.mcp.removed", { name }))) {
      setConfirmRemove("")
      forgetMcpLogo(name)
    }
  }
  const runtime = (server: McpServerStatus, action: RuntimeAction) =>
    void run(server.name, () => (action === "connect" ? engine.actions.mcpConnect(server.name, here()) : engine.actions.mcpDisconnect(server.name, here())))
  const signIn = (name: string) =>
    void run(name, async () => openExternal(await engine.actions.mcpSignIn(name)), t("drift.mcp.signInOpened", { name }))
  const signOut = (name: string) => void run(name, () => engine.actions.mcpSignOut(name))
  /** Installs and connects; a server that answers with a sign-in request has its sign-in page opened at once. */
  const install = async (server: RegistryServer, config: McpServerConfig) => {
    const name = registryInstallName(server)
    rememberMcpLogo(name, server.listing?.image)
    const done = await run(name, async () => {
      const status = await engine.actions.mcpSave(name, config, { create: true, directory: here() })
      if (status.needsSignIn) openExternal(await engine.actions.mcpSignIn(name))
      setMessage(t(status.needsSignIn ? "drift.mcp.signInOpened" : "drift.mcp.installed", { name: server.title ?? name }))
    })
    if (done) {
      setView("servers")
      setSelected(name)
    }
    return done
  }

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
                    workspaceId={hereId()}
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
                    onEnabled={(enabled) => void run(name, () => engine.actions.mcpSetEnabled(name, enabled, here()))}
                    onRuntime={(action) => runtime(server(), action)}
                    onSignIn={() => signIn(name)}
                    onSignOut={() => signOut(name)}
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
        <McpRegistry disabled={locked()} installed={new Set(rowNames())} onInstall={install} />
      </Show>
      <Show when={editor()}>
        {(entry) => (
          <McpEditor
            server={entry().server ? { name: entry().server!.name, config: entry().server!.config, readOnlyTrusted: entry().server!.readOnlyTrusted } : undefined}
            pending={!!busy()}
            onClose={() => setEditor(null)}
            onSave={save}
          />
        )}
      </Show>
    </div>
  )
}

/** How the engine talks to a server: its transport, and once connected the protocol version it agreed to and whether that is stateless. */
export function mcpProtocolLabel(server: Pick<McpServerStatus, "transport" | "protocol" | "era">) {
  const parts = [t(`drift.mcp.transport.${server.transport}`), server.protocol, server.era && t(`drift.mcp.era.${server.era}`)]
  return parts.filter(Boolean).join(" · ")
}

function ServerRow(props: {
  server: McpServerStatus
  workspaceId?: string
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
  onSignIn: () => void
  onSignOut: () => void
}) {
  const status = () => mcpStatusLabel(props.server, props.busy, props.workspaceId)
  const runtime = () => mcpRuntimeAction(props.server, props.workspaceId)
  const scope = () => mcpScopeLabel(props.server, props.workspaceId)
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
        const action = props.disabled ? undefined : mcpRuntimeKeyAction(props.server, event.key, props.workspaceId)
        if (!action) return
        event.preventDefault()
        props.onRuntime(action)
      }}
    >
      <div class="flex items-start gap-3">
        <LogoTile image={mcpLogos()[props.server.name]} title={props.server.name} />
        <div class="min-w-0 flex-1">
          <div class="truncate text-sm font-medium text-ink">{props.server.name}</div>
          <div class="mt-0.5 flex flex-wrap items-center gap-x-2 text-xs">
            <span class={status().tone}>{status().text}</span>
            <Show when={scope()}>{(text) => <span class="text-ink-muted">{text()}</span>}</Show>
            <span class="text-ink-faint">{mcpProtocolLabel(props.server)}</span>
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
          <Show when={props.server.needsSignIn}>
            <Action disabled={props.disabled} title={t("drift.mcp.signIn")} onClick={props.onSignIn}>
              {t("drift.mcp.signIn")}
            </Action>
          </Show>
          <Show when={props.server.signedIn}>
            <Action disabled={props.disabled} title={t("drift.mcp.signOut")} onClick={props.onSignOut}>
              {t("drift.mcp.signOut")}
            </Action>
          </Show>
          <Action disabled={props.disabled} title={t("common.edit")} onClick={props.onEdit}>
            <IconSquarePen class="size-3.5" />
          </Action>
          {/* Always in its place so rows line up; in a workspace it turns the server on or off there only. */}
          <Action
            disabled={props.disabled || !runtime()}
            title={mcpRuntimeTitle(runtime(), props.workspaceId)}
            onClick={() => {
              const action = runtime()
              if (action) props.onRuntime(action)
            }}
          >
            {runtime() === "disconnect" ? <IconPlugOff class="size-3.5" /> : <IconPlug class="size-3.5" />}
          </Action>
          <Toggle
            label={t("drift.mcp.enable", { name: props.server.name })}
            title={t("drift.mcp.switchHint")}
            checked={props.server.enabled}
            disabled={props.disabled || props.server.unreadable}
            onChange={() => props.onEnabled(!props.server.enabled)}
          />
        </div>
      </div>
    </div>
  )
}

/** The plug button's words: in a workspace it turns the server on or off there; with none open, it connects or disconnects. */
function mcpRuntimeTitle(action: RuntimeAction | undefined, workspaceId?: string) {
  if (workspaceId === undefined) return t(action === "disconnect" ? "common.disconnect" : "common.connect")
  return t(action === "disconnect" ? "drift.mcp.offHere" : "drift.mcp.onHere")
}

/** A server whose workspaces chose otherwise than its switch says where it is on, as seen from `workspaceId`. */
export function mcpScopeLabel(server: Pick<McpServerStatus, "enabled" | "workspaces">, workspaceId?: string) {
  if (workspaceId === undefined || server.enabled) return undefined
  if (mcpOnIn(server, workspaceId)) return t("drift.mcp.scope.chosen")
  return server.workspaces.some((choice) => choice.enabled) ? t("drift.mcp.scope.elsewhere") : undefined
}

export function mcpStatusLabel(server: McpServerStatus, busy: boolean, workspaceId?: string) {
  if (busy) return { text: t("common.loading"), tone: "text-ink-faint" }
  if (workspaceId !== undefined && server.state !== "disabled" && !mcpOnIn(server, workspaceId)) return { text: t("drift.mcp.status.offHere"), tone: "text-ink-faint" }
  switch (server.state) {
    case "connected":
      return { text: t("mcp.status.connected"), tone: "text-ok" }
    case "connecting":
      return { text: t("drift.mcp.status.connecting"), tone: "text-ink-faint" }
    case "disconnected":
      return { text: t("drift.mcp.status.disconnected"), tone: "text-ink-faint" }
    case "failed":
      if (server.needsSignIn) return { text: t("drift.mcp.status.needsSignIn"), tone: "text-warn" }
      return { text: server.error || t("mcp.status.failed"), tone: "text-danger" }
    case "disabled":
      return { text: t("mcp.status.disabled"), tone: "text-ink-faint" }
  }
}

function errorText(error: unknown) {
  return error instanceof Error ? error.message : String(error)
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
