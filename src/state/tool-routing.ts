import { createSignal } from "solid-js"
import { backendInvoke } from "../backend"
import { shellEvents } from "../shell"

export type ToolRoutingStatus = { outcome: string; at: number; hidden?: number; httpStatus?: number }

const [policy, setPolicy] = createSignal({ enabled: false })
const [status, setStatus] = createSignal<ToolRoutingStatus | null>(null)
export const toolRouting = policy
export const toolRoutingStatus = status

export function listenToolRouting() {
  return shellEvents()?.listen<{ enabled: boolean }>("tool-routing-changed", (event) => {
    setPolicy(event.payload)
    setStatus(null)
  })
}

export async function loadToolRouting() {
  const invoke = backendInvoke()
  if (!invoke) return
  setPolicy(await invoke<{ enabled: boolean }>("tool_routing_snapshot"))
}

export async function loadToolRoutingStatus() {
  const invoke = backendInvoke()
  if (!invoke) return
  setStatus(await invoke<ToolRoutingStatus | null>("tool_routing_status"))
}

export async function setToolRouting(enabled: boolean) {
  const invoke = backendInvoke()
  if (!invoke) throw new Error("Tool routing requires the Drift host backend")
  setPolicy(await invoke<{ enabled: boolean }>("tool_routing_update", { policy: { enabled } }))
  setStatus(null)
}
