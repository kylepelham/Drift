import { createSignal } from "solid-js"
import { backendInvoke } from "../backend"
import { shellEvents } from "../shell"

const [policy, setPolicy] = createSignal({ enabled: false })
export const toolRouting = policy

export function listenToolRouting() {
  return shellEvents()?.listen<{ enabled: boolean }>("tool-routing-changed", (event) => setPolicy(event.payload))
}

export async function loadToolRouting() {
  const invoke = backendInvoke()
  if (!invoke) return
  setPolicy(await invoke<{ enabled: boolean }>("tool_routing_snapshot"))
}

export async function setToolRouting(enabled: boolean) {
  const invoke = backendInvoke()
  if (!invoke) throw new Error("Tool routing requires the Drift host backend")
  setPolicy(await invoke<{ enabled: boolean }>("tool_routing_update", { policy: { enabled } }))
}
