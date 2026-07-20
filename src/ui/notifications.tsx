import { createEffect, untrack } from "solid-js"
import type { Engine } from "../engine"
import { notifyAttention } from "../state/prefs"
import { selectSession } from "../state/selection"
import { shellInvoke } from "../state/store"

// WebView2 stubs the Web Notification API, so the shell path uses the Tauri plugin.
function show(sessionId: string, title: string, body: string) {
  if (!notifyAttention() || document.hasFocus()) return
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
    const all = Object.values(props.engine.state.permissions).flat()
    const present = new Set(all.map((permission) => permission.id))
    for (const id of seen) if (!present.has(id)) seen.delete(id)
    for (const permission of all) {
      if (seen.has(permission.id)) continue
      seen.add(permission.id)
      untrack(() => {
        const session = props.engine.state.sessions[permission.sessionID]
        show(permission.sessionID, "Drift needs permission", `${permission.title} · ${session?.title ?? "thread"}`)
      })
    }
  })

  const busy = new Set<string>()
  createEffect(() => {
    for (const [sessionId, status] of Object.entries(props.engine.state.status)) {
      const running = status.type === "busy" || status.type === "retry"
      if (running) busy.add(sessionId)
      else if (busy.delete(sessionId))
        untrack(() => {
          const session = props.engine.state.sessions[sessionId]
          show(sessionId, "Drift finished", session?.title || "A thread finished working")
        })
    }
  })
  return null
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
