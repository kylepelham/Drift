import { shellInvoke, type ShellInvoke } from "../shell"

export type Connection = "idle" | "connecting" | "online" | "offline"
export type EngineTarget = { url: string; headers?: Record<string, string> }

export type ShellEngineStatus = { url?: string; error?: string; password?: string }
export type ShellEngineInspection = { target?: EngineTarget; error?: string }

// How long to wait for the shell to report a ready embedded engine before giving up.
const engineReadyTimeoutMs = 45_000
const engineReadyPollMs = 300
// The embedded engine is always addressed with this fixed username; only the password varies.
const engineUsername = "opencode"

export async function resolveEngine(): Promise<EngineTarget> {
  const invoke = shellInvoke()
  if (invoke) return waitForShellEngine(invoke)
  return {
    url: import.meta.env.VITE_ENGINE_URL ?? "http://127.0.0.1:4096",
    headers: basicAuth(import.meta.env.VITE_ENGINE_USERNAME, import.meta.env.VITE_ENGINE_PASSWORD),
  }
}

export async function inspectShellEngine(): Promise<ShellEngineInspection | undefined> {
  const invoke = shellInvoke()
  if (!invoke) return undefined
  return inspectStatus(await invoke<ShellEngineStatus>("engine_status"))
}

export async function restartShellEngine(): Promise<EngineTarget> {
  const invoke = shellInvoke()
  if (!invoke) throw new Error("embedded engine restart is unavailable")
  await invoke("restart_engine")
  return waitForShellEngine(invoke)
}

export async function configureShellTimeout(target: EngineTarget, timeoutMs: number | null) {
  const response = await fetch(`${target.url}/experimental/control-plane/shell-timeout`, {
    method: "PUT",
    headers: { "content-type": "application/json", ...target.headers },
    body: JSON.stringify({ timeout: timeoutMs }),
  })
  if (!response.ok) throw new Error(`engine rejected shell timeout (${response.status})`)
}

export function inspectStatus(status: ShellEngineStatus): ShellEngineInspection {
  if (status.url) return { target: { url: status.url, headers: basicAuth(engineUsername, status.password) } }
  if (status.error) return { error: status.error }
  return {}
}

async function waitForShellEngine(invoke: ShellInvoke): Promise<EngineTarget> {
  const deadline = Date.now() + engineReadyTimeoutMs
  for (;;) {
    const status = inspectStatus(await invoke<ShellEngineStatus>("engine_status"))
    if (status.target) return status.target
    if (status.error) throw new Error(status.error)
    if (Date.now() >= deadline) {
      throw new Error(`embedded engine did not become ready within ${engineReadyTimeoutMs / 1000} seconds`)
    }
    await sleep(engineReadyPollMs)
  }
}

function basicAuth(username?: string, password?: string) {
  if (!password) return undefined
  return { Authorization: `Basic ${btoa(`${username ?? engineUsername}:${password}`)}` }
}

export function sleep(ms: number) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}
