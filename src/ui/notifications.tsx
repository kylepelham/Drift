import { createEffect, createMemo, createSignal, For, onCleanup, Show, untrack } from "solid-js"
import { useEngine, type Engine } from "../engine"
import {
  alertSounds,
  customSound,
  systemNotifications,
  type AttentionKind,
} from "../state/prefs"
import { permissionRequiresAttention } from "../state/permission-attention"
import { selectSession } from "../state/selection"
import { t } from "../state/i18n"
import {
  exactMcpTarget,
  mcpCoordinator,
  mcpSnapshotActionable,
  type McpCoordinatorState,
  type McpExactTarget,
} from "../state/mcp"
import { shellInvoke } from "../shell"
import { openMcpServers } from "./mcp"
import { playAlertSound } from "./sounds"

// WebView2 stubs the Web Notification API, so the shell path uses the Tauri plugin.
function show(kind: AttentionKind, sessionId: string, title: string, body: string) {
  void playAlertSound(alertSounds()[kind] ?? "none", customSound())
  if (!systemNotifications()[kind] || document.hasFocus()) return
  const invoke = shellInvoke()
  if (invoke) {
    void invoke("plugin:notification|notify", { options: { title, body } }).catch(() => {})
    return
  }
  if (typeof Notification === "undefined" || Notification.permission !== "granted") return
  const notification = new Notification(title, { body, tag: `drift-${sessionId}` })
  notification.onclick = () => {
    window.focus()
    selectSession(sessionId)
    notification.close()
  }
}

export function AttentionNotifier(props: { engine: Engine }) {
  const seen = new Set<string>()
  createEffect(() => {
    const pending = Object.values(props.engine.state.permissions).flat()
    const all = pending.filter((permission) => permissionRequiresAttention(permission, props.engine.state))
    const present = new Set(pending.map((permission) => permission.id))
    for (const id of seen) if (!present.has(id)) seen.delete(id)
    for (const permission of all) {
      if (seen.has(permission.id)) continue
      seen.add(permission.id)
      untrack(() => {
        const session = props.engine.state.sessions[permission.sessionID]
        show(
          "permission",
          permission.sessionID,
          t("notification.permission.title"),
          `${permission.title} · ${session?.title ?? t("drift.thread.untitled")}`,
        )
      })
    }
  })

  const seenQuestions = new Set<string>()
  createEffect(() => {
    const all = Object.values(props.engine.state.questions).flat()
    const present = new Set(all.map((question) => question.id))
    for (const id of seenQuestions) if (!present.has(id)) seenQuestions.delete(id)
    for (const question of all) {
      if (seenQuestions.has(question.id)) continue
      seenQuestions.add(question.id)
      untrack(() => {
        const session = props.engine.state.sessions[question.sessionID]
        show(
          "agent",
          question.sessionID,
          t("notification.question.title"),
          question.questions[0]?.header ?? session?.title ?? t("drift.thread.untitled"),
        )
      })
    }
  })

  const busy = new Set<string>()
  createEffect(() => {
    for (const [sessionId, status] of Object.entries(props.engine.state.status)) {
      const running = status.type === "busy" || status.type === "retry"
      if (running) busy.add(sessionId)
      else if (busy.delete(sessionId) && !props.engine.state.errors[sessionId])
        untrack(() => {
          const session = props.engine.state.sessions[sessionId]
          show(
            "agent",
            sessionId,
            t("notification.session.responseReady.title"),
            session?.title || t("drift.notification.threadFinished"),
          )
        })
    }
  })

  const errors = new Map<string, string>()
  createEffect(() => {
    for (const [sessionId, error] of Object.entries(props.engine.state.errors)) {
      if (errors.get(sessionId) === error) continue
      errors.set(sessionId, error)
      untrack(() => {
        const session = props.engine.state.sessions[sessionId]
        show(
          "error",
          sessionId,
          t("notification.session.error.title"),
          session?.title ? t("drift.notification.threadError", { title: session.title, error }) : error,
        )
      })
    }
    for (const id of errors.keys()) if (!props.engine.state.errors[id]) errors.delete(id)
  })
  return null
}

export function NoticeHost() {
  const engine = useEngine()
  const [now, setNow] = createSignal(Date.now())
  const [dismissed, setDismissed] = createSignal(new Set<string>())
  const [hiddenMcp, setHiddenMcp] = createSignal<ReadonlySet<string>>(new Set())
  const timer = setInterval(() => setNow(Date.now()), 250)
  onCleanup(() => clearInterval(timer))
  const visible = createMemo(() =>
    engine.state.notices.filter((notice) => !dismissed().has(notice.id) && notice.created + notice.duration > now()),
  )
  createEffect(() => {
    const active = new Set(
      engine.state.notices.filter((notice) => notice.created + notice.duration > now()).map((notice) => notice.id),
    )
    setDismissed((current) => pruneDismissedNoticeIds(current, active))
  })
  const dismiss = (id: string) => setDismissed((current) => new Set([...current, id]))
  const pendingMcp = createMemo(() => mcpPromptTargets(mcpCoordinator.state))
  const mcpBusy = () => !!mcpCoordinator.state.mutation || !mcpSnapshotActionable(mcpCoordinator.state)
  createEffect(() => {
    const present = new Set(pendingMcp().map(mcpPromptKey))
    setHiddenMcp((current) => new Set([...current].filter((key) => present.has(key))))
  })
  const decide = (action: "approve" | "reject", target: McpExactTarget) => {
    const key = mcpPromptKey(target)
    setHiddenMcp((current) => reduceMcpPromptState(current, { type: "start", key }))
    void mcpCoordinator.decide(action, target).catch((error: unknown) => {
      setHiddenMcp((current) => reduceMcpPromptState(current, { type: "failed", key }))
      engine.actions.notice({
        id: nextNoticeOccurrenceId(`mcp-${action}-failed:${key}`),
        title: t("drift.mcp.toast.failed"),
        message: error instanceof Error ? error.message : String(error),
        variant: "error",
      })
    })
  }
  return (
    <div
      class="pointer-events-none fixed top-11 right-5 bottom-5 z-[80] flex w-[min(24rem,calc(100vw-2.5rem))]"
    >
      <div
        class="pointer-events-auto mt-auto flex max-h-full min-h-0 w-full flex-col gap-2 overflow-y-auto overscroll-contain pr-1"
        aria-live="polite"
      >
        <Show when={engine.state.engineError}>
          <div class="rounded-lg border border-danger/40 bg-surface/95 px-3 py-2 text-danger shadow-xl backdrop-blur" role="alert">
            <div class="text-sm font-semibold">{t("drift.engine.stopped.title")}</div>
            <div class="mt-0.5 text-sm break-words">{engine.state.engineError}</div>
            <button
              class="mt-2 rounded-md border border-danger/40 px-2 py-1 text-xs hover:bg-danger/10 disabled:opacity-40"
              disabled={engine.state.engineRestarting}
              onClick={() => void engine.restartEngine()}
            >
              {engine.state.engineRestarting ? t("drift.engine.restarting") : t("drift.engine.restart")}
            </button>
          </div>
        </Show>
        <For each={pendingMcp().filter((target) => !hiddenMcp().has(mcpPromptKey(target)))}>
          {(target) => (
            <div class="rounded-lg border border-warn/40 bg-surface/95 px-3 py-2 shadow-xl backdrop-blur" role="status">
              <div class="text-sm font-semibold text-ink">{t("drift.mcp.toast.pending.title")}</div>
              <div class="mt-0.5 text-sm text-ink">
                {t("drift.mcp.toast.pending.message", { name: target.name })}
              </div>
              <div class="mt-1 font-mono text-[0.68rem] text-ink-faint">
                {t("drift.mcp.toast.exact", { id: target.fingerprint.replace(/^sha256:/, "").slice(0, 12) })}
              </div>
              <div class="mt-2 flex flex-wrap gap-1.5">
                <button
                  class="rounded-md border border-warn/40 px-2 py-1 text-xs text-warn hover:bg-warn/10 disabled:opacity-40"
                  disabled={mcpBusy()}
                  onClick={() => decide("approve", target)}
                >
                  {t("drift.mcp.approve")}
                </button>
                <button
                  class="rounded-md border border-edge px-2 py-1 text-xs text-ink-muted hover:text-ink disabled:opacity-40"
                  disabled={mcpBusy()}
                  onClick={() => decide("reject", target)}
                >
                  {t("drift.mcp.reject")}
                </button>
                <button
                  class="rounded-md border border-accent/40 px-2 py-1 text-xs text-accent hover:bg-accent/10"
                  onClick={openMcpServers}
                >
                  {t("drift.mcp.toast.openSettings")}
                </button>
              </div>
            </div>
          )}
        </For>
        <For each={visible()}>
          {(notice) => (
            <div
              class="rounded-lg border bg-surface/95 px-3 py-2 shadow-xl backdrop-blur"
              classList={{
                "border-edge-strong text-ink": notice.variant === "info",
                "border-ok/40 text-ok": notice.variant === "success",
                "border-warn/40 text-warn": notice.variant === "warning",
                "border-danger/40 text-danger": notice.variant === "error",
              }}
              role={notice.variant === "error" ? "alert" : "status"}
            >
              <div class="flex items-start gap-3">
                <div class="min-w-0 flex-1">
                  <Show when={notice.title}>{(title) => <div class="text-sm font-semibold">{title()}</div>}</Show>
                  <div class="text-sm break-words" classList={{ "mt-0.5": !!notice.title }}>
                    {notice.message}
                  </div>
                </div>
                <button class="shrink-0 text-xs opacity-60 hover:opacity-100" onClick={() => dismiss(notice.id)}>
                  {t("common.dismiss")}
                </button>
              </div>
            </div>
          )}
        </For>
      </div>
    </div>
  )
}

export type McpPromptEvent = { type: "start" | "failed"; key: string }

let noticeOccurrence = 0

export function nextNoticeOccurrenceId(base: string) {
  return `${base}:${Date.now()}:${noticeOccurrence++}`
}

export function pruneDismissedNoticeIds(dismissed: Set<string>, active: ReadonlySet<string>): Set<string> {
  const next = new Set([...dismissed].filter((id) => active.has(id)))
  if (next.size === dismissed.size && [...next].every((id) => dismissed.has(id))) return dismissed
  return next
}

export function reduceMcpPromptState(state: ReadonlySet<string>, event: McpPromptEvent) {
  const next = new Set(state)
  if (event.type === "start") next.add(event.key)
  else next.delete(event.key)
  return next
}

export function mcpPromptTargets(state: Pick<McpCoordinatorState, "directory" | "snapshot">) {
  if (state.snapshot.directory !== state.directory) return []
  return state.snapshot.observed
    .filter((item) => item.decision === "pending")
    .map((item) => exactMcpTarget(state.snapshot, item))
}

export function mcpPromptKey(target: McpExactTarget) {
  return `${target.generation}:${target.directory}:${target.name}:${target.fingerprint}`
}

export function requestNotificationPermission() {
  const invoke = shellInvoke()
  if (invoke) {
    void invoke<boolean>("plugin:notification|is_permission_granted")
      .then((granted) => (granted ? undefined : invoke("plugin:notification|request_permission")))
      .catch(() => {})
    return
  }
  if (typeof Notification === "undefined") return
  if (Notification.permission === "default") void Notification.requestPermission()
}
