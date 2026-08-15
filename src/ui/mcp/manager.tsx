import type { McpStatus } from "@opencode-ai/sdk/client"
import { createEffect, createMemo, createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js"
import { registryConfig, type RegistryServer } from "../../mcp-registry"
import { createRegistrySearch } from "../../state/mcp-registry-search"
import {
  exactMcpTarget,
  mcpCoordinator,
  mcpFingerprintId,
  mcpSnapshotActionable,
  type McpExactTarget,
  type McpStoredExpectation,
} from "../../state/mcp"
import { t } from "../../state/i18n"
import { backendInvoke } from "../../backend"
import type { McpConfig, ObservedMcpServer, StoredMcpServer } from "../../state/store"
import { IconCheck, IconPlus, IconShieldCheck, IconSquarePen } from "../icons"
import { McpEditor } from "./editor"

type Row = { name: string; stored?: StoredMcpServer; observed?: ObservedMcpServer; status?: McpStatus }
/** `external` marks a server defined in the user's own config files rather than Drift's registry. */
type EditorEntry = { server?: StoredMcpServer; expected: McpStoredExpectation; external?: McpExactTarget }
type RuntimeAction = "connect" | "disconnect" | "authenticate"
type RowKey = "ArrowUp" | "ArrowDown" | "Home" | "End"

export function mcpRuntimeAction(status: McpStatus): RuntimeAction {
  if (status.status === "connected") return "disconnect"
  if (status.status === "needs_auth" || status.status === "needs_client_registration") return "authenticate"
  return "connect"
}

export function mcpRuntimeKeyAction(status: McpStatus, key: string): RuntimeAction | undefined {
  if (key === "ArrowLeft") return status.status === "connected" ? "disconnect" : undefined
  if (key === "ArrowRight") return status.status === "connected" ? undefined : mcpRuntimeAction(status)
  if (key === "Enter") return mcpRuntimeAction(status)
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
  const coordinator = mcpCoordinator
  const [editor, setEditor] = createSignal<EditorEntry | null>(null)
  const [view, setView] = createSignal<"servers" | "registry">("servers")
  const [confirmRemove, setConfirmRemove] = createSignal("")
  const [message, setMessage] = createSignal("")
  // Failures outside the coordinator's mutation path (external config lookup) surface here.
  const [failure, setFailure] = createSignal("")
  const [selected, setSelected] = createSignal("")
  const rowElements = new Map<string, HTMLDivElement>()
  const native = !!backendInvoke()
  const locked = () => !native || !!coordinator.state.mutation || !mcpSnapshotActionable(coordinator.state)
  const rows = createMemo<Row[]>(() => {
    const result = new Map<string, Row>()
    for (const stored of coordinator.state.snapshot.servers) result.set(stored.name, { name: stored.name, stored })
    for (const observed of coordinator.state.snapshot.observed) {
      const row = result.get(observed.name) ?? { name: observed.name }
      row.observed = observed
      result.set(observed.name, row)
    }
    for (const [name, status] of Object.entries(coordinator.state.statuses)) {
      const row = result.get(name) ?? { name }
      row.status = status
      result.set(name, row)
    }
    return [...result.values()].sort((a, b) => a.name.localeCompare(b.name))
  })
  const rowNames = createMemo(() => rows().map((row) => row.name))
  const exact = (observed: ObservedMcpServer) => exactMcpTarget(coordinator.state.snapshot, observed)
  const moveRow = (key: RowKey, current = selected()) => {
    const next = nextMcpRowName(
      rows().map((row) => row.name),
      current,
      key,
    )
    if (!next) return
    setSelected(next)
    rowElements.get(next)?.focus()
  }
  createEffect(() => {
    const names = new Set(rows().map((row) => row.name))
    for (const name of rowElements.keys()) if (!names.has(name)) rowElements.delete(name)
    if (!names.has(selected())) setSelected(rows()[0]?.name ?? "")
  })
  onMount(() => {
    void coordinator.refreshStatus().catch(() => undefined)
    const timer = window.setInterval(() => void coordinator.refreshStatus().catch(() => undefined), 2_000)
    onCleanup(() => window.clearInterval(timer))
  })
  const run = async (action: () => Promise<void>, success?: string) => {
    setMessage("")
    setFailure("")
    try {
      await action()
      if (success) setMessage(success)
      return true
    } catch {
      return false
    }
  }
  const save = async (name: string, config: McpConfig, expected: McpStoredExpectation) => {
    setMessage("")
    setFailure("")
    const external = editor()?.external
    if (external) await coordinator.saveExternal(external, name, config)
    else await coordinator.save(name, config, expected)
    setMessage(t("drift.mcp.saved", { name }))
    setEditor(null)
  }
  const remove = async (server: StoredMcpServer) => {
    if (confirmRemove() !== server.name) return setConfirmRemove(server.name)
    const expected = {
      generation: coordinator.state.snapshot.generation,
      previousName: server.name,
      updatedAt: server.updatedAt,
    }
    if (await run(() => coordinator.remove(server.name, expected), t("drift.mcp.removed", { name: server.name })))
      setConfirmRemove("")
  }
  const editExternal = async (target: McpExactTarget) => {
    setMessage("")
    setFailure("")
    try {
      const found = await coordinator.externalConfig(target)
      setEditor({
        server: { name: target.name, config: found.config, updatedAt: 0 },
        expected: { generation: coordinator.state.snapshot.generation },
        external: target,
      })
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error))
    }
  }
  const removeExternal = async (target: McpExactTarget) => {
    if (confirmRemove() !== target.name) return setConfirmRemove(target.name)
    if (await run(() => coordinator.removeExternal(target), t("drift.mcp.removed", { name: target.name })))
      setConfirmRemove("")
  }
  const decide = (action: "approve" | "reject" | "revoke", target: McpExactTarget) =>
    void run(
      () => coordinator.decide(action, target),
      t(
        action === "approve" ? "drift.mcp.approved" : action === "reject" ? "drift.mcp.rejected" : "drift.mcp.revoked",
        { name: target.name },
      ),
    )

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
            onClick={() => setEditor({ expected: { generation: coordinator.state.snapshot.generation } })}
          >
            <IconPlus class="size-3.5" />
            {t("drift.mcp.add")}
          </button>
        </Show>
      </div>
      <Show when={coordinator.state.error || failure() || message()}>
        <div
          role={coordinator.state.error || failure() ? "alert" : "status"}
          class="rounded-md border px-3 py-2 text-xs"
          classList={{
            "border-danger/35 bg-danger/10 text-danger": !!(coordinator.state.error || failure()),
            "border-ok/35 bg-ok/10 text-ok": !coordinator.state.error && !failure(),
          }}
        >
          {coordinator.state.error || failure() || message()}
        </div>
      </Show>
      <Show when={!coordinator.state.directory}>
        <div class="text-xs text-ink-faint">{t("drift.mcp.selectWorkspace")}</div>
      </Show>
      <Show when={view() === "servers"}>
        <div classList={{ "space-y-1": !props.embedded, "border-y border-edge/80": props.embedded }}>
          <For each={rowNames()}>
            {(name) => {
              const row = () => rows().find((item) => item.name === name)!
              return (
                <ServerRow
                  row={row()}
                  selected={selected() === name}
                  target={row().observed ? exact(row().observed!) : undefined}
                  embedded={props.embedded}
                  disabled={locked()}
                  busy={coordinator.state.mutation === name}
                  confirming={confirmRemove() === name}
                  rowRef={(element) => rowElements.set(name, element)}
                  onFocus={() => setSelected(name)}
                  onNavigate={(key) => moveRow(key, name)}
                  onEdit={() => {
                    const stored = row().stored
                    if (stored) {
                      setEditor({
                        server: stored,
                        expected: {
                          generation: coordinator.state.snapshot.generation,
                          previousName: stored.name,
                          updatedAt: stored.updatedAt,
                        },
                      })
                      return
                    }
                    const observed = row().observed
                    if (observed) void editExternal(exact(observed))
                  }}
                  onRemove={() => {
                    const stored = row().stored
                    if (stored) {
                      void remove(stored)
                      return
                    }
                    const observed = row().observed
                    if (observed) void removeExternal(exact(observed))
                  }}
                  onDecision={decide}
                  onRuntime={(action) => {
                    const observed = row().observed
                    if (observed) void run(() => coordinator.runtime(exact(observed), action))
                  }}
                />
              )
            }}
          </For>
          <Show when={!coordinator.state.loading && !rows().length}>
            <div class="px-3 py-5 text-sm text-ink-faint">{t("dialog.mcp.empty")}</div>
          </Show>
        </div>
      </Show>
      <Show when={view() === "registry"}>
        <McpRegistry
          embedded={props.embedded}
          disabled={locked()}
          installed={new Set(coordinator.state.snapshot.servers.map((item) => item.name))}
          onInstall={(server) => {
            const config = registryConfig(server)
            if (!config) return setMessage(t("drift.mcp.registryUnavailable"))
            void run(
              () => coordinator.save(server.name, config, { generation: coordinator.state.snapshot.generation }),
              t("drift.mcp.installed", { name: server.title ?? server.name }),
            )
          }}
        />
      </Show>
      <Show when={editor()}>
        {(entry) => (
          <McpEditor
            server={entry().server}
            expected={entry().expected}
            pending={!!coordinator.state.mutation}
            onClose={() => setEditor(null)}
            onSave={save}
          />
        )}
      </Show>
    </div>
  )
}

function ServerRow(props: {
  row: Row
  selected: boolean
  target?: McpExactTarget
  embedded?: boolean
  disabled: boolean
  busy: boolean
  confirming: boolean
  rowRef: (element: HTMLDivElement) => void
  onFocus: () => void
  onNavigate: (key: RowKey) => void
  onEdit: () => void
  onRemove: () => void
  onDecision: (action: "approve" | "reject" | "revoke", target: McpExactTarget) => void
  onRuntime: (action: RuntimeAction) => void
}) {
  const status = () => statusLabel(props.row, props.busy)
  const keyboardAction = (key: string) => {
    if (props.disabled || props.target?.decision !== "approved" || !props.row.status) return
    const action = mcpRuntimeKeyAction(props.row.status, key)
    if (action) props.onRuntime(action)
    return action
  }
  return (
    <div
      ref={props.rowRef}
      data-mcp-row={props.row.name}
      tabIndex={props.selected ? 0 : -1}
      aria-label={props.row.name}
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
        if (keyboardAction(event.key)) event.preventDefault()
      }}
    >
      <div class="flex items-start gap-3">
        <div class="min-w-0 flex-1">
          <div class="truncate text-sm font-medium text-ink">{props.row.name}</div>
          <div class="mt-0.5 flex flex-wrap items-center gap-x-2 text-xs">
            <Show when={props.row.observed}>
              {(item) => (
                <span class="font-mono text-ink-faint" title={item().fingerprint}>
                  {mcpFingerprintId(item().fingerprint)}
                </span>
              )}
            </Show>
            <span class={status().tone}>{status().text}</span>
          </div>
        </div>
        <div class="flex shrink-0 flex-wrap justify-end gap-1.5">
          <Show when={props.target?.decision === "pending" && props.target}>
            {(target) => (
              <>
                <Action disabled={props.disabled} tone="warn" onClick={() => props.onDecision("approve", target())}>
                  <IconShieldCheck class="size-3.5" />
                  {t("drift.mcp.approve")}
                </Action>
                <Action disabled={props.disabled} onClick={() => props.onDecision("reject", target())}>
                  {t("drift.mcp.reject")}
                </Action>
              </>
            )}
          </Show>
          <Show when={props.target?.decision === "approved" || props.target?.decision === "rejected"}>
            <Action disabled={props.disabled} onClick={() => props.onDecision("revoke", props.target!)}>
              {t("drift.mcp.revoke")}
            </Action>
          </Show>
          <Show when={props.target?.decision === "approved" && props.row.status}>
            <Runtime status={props.row.status!} disabled={props.disabled} onRun={props.onRuntime} />
          </Show>
          <Show when={props.row.stored || props.target}>
            <Action disabled={props.disabled} onClick={props.onEdit}>
              <IconSquarePen class="size-3" />
              {t("common.edit")}
            </Action>
            <Action disabled={props.disabled} tone="danger" onClick={props.onRemove}>
              {props.confirming ? t("drift.mcp.confirmRemove") : t("drift.mcp.remove")}
            </Action>
          </Show>
        </div>
      </div>
    </div>
  )
}

function Runtime(props: {
  status: McpStatus
  disabled: boolean
  onRun: (action: RuntimeAction) => void
}) {
  const action = () => mcpRuntimeAction(props.status)
  return (
    <Action disabled={props.disabled} onClick={() => props.onRun(action())}>
      {t(action() === "authenticate" ? "drift.mcp.authenticate" : `common.${action()}`)}
    </Action>
  )
}

function statusLabel(row: Row, busy: boolean) {
  if (busy) return { text: t("common.loading"), tone: "text-ink-faint" }
  if (row.observed?.decision === "invalid") return { text: t("drift.mcp.invalidStatus"), tone: "text-danger" }
  if (row.observed?.decision === "pending") return { text: t("drift.mcp.pendingApproval"), tone: "text-warn" }
  if (row.observed?.decision === "rejected") return { text: t("drift.mcp.rejectedStatus"), tone: "text-danger" }
  if (!row.observed && row.stored) return { text: t("drift.mcp.awaitingReport"), tone: "text-ink-faint" }
  if (!row.status)
    return { text: row.observed?.decision === "approved" ? t("mcp.status.disabled") : "", tone: "text-ink-faint" }
  if (row.status.status === "connected") return { text: t("mcp.status.connected"), tone: "text-ok" }
  if (row.status.status === "failed")
    return {
      text: "error" in row.status && row.status.error ? String(row.status.error) : t("mcp.status.failed"),
      tone: "text-danger",
    }
  if (row.status.status === "needs_auth") return { text: t("mcp.status.needs_auth"), tone: "text-warn" }
  if (row.status.status === "needs_client_registration")
    return { text: t("mcp.status.needs_client_registration"), tone: "text-warn" }
  return { text: t("mcp.status.disabled"), tone: "text-ink-faint" }
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
                <Action
                  disabled={props.disabled || props.installed.has(server.name)}
                  onClick={() => props.onInstall(server)}
                >
                  {props.installed.has(server.name) ? <IconCheck class="size-3.5" /> : <IconPlus class="size-3.5" />}
                  {t(props.installed.has(server.name) ? "drift.mcp.installedLabel" : "drift.mcp.install")}
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
      class="w-full rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-ink outline-none placeholder:text-ink-faint focus:border-edge-strong"
      placeholder={props.label}
      value={props.value}
      onInput={(event) => props.onInput(event.currentTarget.value)}
    />
  )
}

function Action(props: { disabled?: boolean; tone?: "warn" | "danger"; onClick: () => void; children: JSX.Element }) {
  return (
    <button
      type="button"
      disabled={props.disabled}
      class="flex items-center gap-1 rounded-md border px-2 py-1 text-xs disabled:opacity-40"
      classList={{
        "border-edge text-ink-muted hover:text-ink": !props.tone,
        "border-warn/40 text-warn hover:bg-warn/10": props.tone === "warn",
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
