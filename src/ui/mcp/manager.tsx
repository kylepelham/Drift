import type { McpStatus } from "@opencode-ai/sdk/client"
import { createMemo, createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js"
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
import { shellInvoke, type McpConfig, type ObservedMcpServer, type StoredMcpServer } from "../../state/store"
import { IconCheck, IconPlus, IconShieldCheck, IconSquarePen } from "../icons"
import { McpEditor } from "./editor"

type Row = { name: string; stored?: StoredMcpServer; observed?: ObservedMcpServer; status?: McpStatus }
type EditorEntry = { server?: StoredMcpServer; expected: McpStoredExpectation }

export function McpManagement(props: { embedded?: boolean }) {
  const coordinator = mcpCoordinator
  const [editor, setEditor] = createSignal<EditorEntry | null>(null)
  const [view, setView] = createSignal<"servers" | "registry">("servers")
  const [confirmRemove, setConfirmRemove] = createSignal("")
  const [message, setMessage] = createSignal("")
  const native = !!shellInvoke()
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
  const exact = (observed: ObservedMcpServer) => exactMcpTarget(coordinator.state.snapshot, observed)
  const run = async (action: () => Promise<void>, success?: string) => {
    setMessage("")
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
    await coordinator.save(name, config, expected)
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
          <Tab active={view() === "servers"} onClick={() => setView("servers")}>
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
      <Show when={coordinator.state.error || message()}>
        <div
          role={coordinator.state.error ? "alert" : "status"}
          class="rounded-md border px-3 py-2 text-xs"
          classList={{
            "border-danger/35 bg-danger/10 text-danger": !!coordinator.state.error,
            "border-ok/35 bg-ok/10 text-ok": !coordinator.state.error,
          }}
        >
          {coordinator.state.error || message()}
        </div>
      </Show>
      <Show when={!coordinator.state.directory}>
        <div class="text-xs text-ink-faint">{t("drift.mcp.selectWorkspace")}</div>
      </Show>
      <Show when={view() === "servers"}>
        <div classList={{ "space-y-1": !props.embedded, "border-y border-edge/80": props.embedded }}>
          <For each={rows()}>
            {(row) => (
              <ServerRow
                row={row}
                target={row.observed ? exact(row.observed) : undefined}
                embedded={props.embedded}
                disabled={locked()}
                busy={coordinator.state.mutation === row.name}
                confirming={confirmRemove() === row.name}
                onEdit={() =>
                  row.stored &&
                  setEditor({
                    server: row.stored,
                    expected: {
                      generation: coordinator.state.snapshot.generation,
                      previousName: row.stored.name,
                      updatedAt: row.stored.updatedAt,
                    },
                  })
                }
                onRemove={() => row.stored && void remove(row.stored)}
                onDecision={decide}
                onRuntime={(action) => {
                  const target = row.observed ? exact(row.observed) : undefined
                  if (target) void run(() => coordinator.runtime(target, action))
                }}
              />
            )}
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
  target?: McpExactTarget
  embedded?: boolean
  disabled: boolean
  busy: boolean
  confirming: boolean
  onEdit: () => void
  onRemove: () => void
  onDecision: (action: "approve" | "reject" | "revoke", target: McpExactTarget) => void
  onRuntime: (action: "connect" | "disconnect" | "authenticate") => void
}) {
  const status = () => statusLabel(props.row, props.busy)
  return (
    <div
      class="px-3 py-2.5 hover:bg-raised/40"
      classList={{
        "rounded-lg border border-transparent hover:border-edge": !props.embedded,
        "border-b border-edge/70 last:border-b-0": props.embedded,
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
          <Show when={props.row.stored}>
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
  onRun: (action: "connect" | "disconnect" | "authenticate") => void
}) {
  const action = () =>
    props.status.status === "connected"
      ? "disconnect"
      : props.status.status === "needs_auth" || props.status.status === "needs_client_registration"
        ? "authenticate"
        : "connect"
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

function Tab(props: { active: boolean; onClick: () => void; children: JSX.Element }) {
  return (
    <button
      type="button"
      aria-pressed={props.active}
      class="min-w-0 flex-1 rounded-md px-2.5 py-1.5 text-xs"
      classList={{ "bg-raised text-ink": props.active, "text-ink-faint hover:text-ink": !props.active }}
      onClick={props.onClick}
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
