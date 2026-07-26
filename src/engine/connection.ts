export type Connection = "idle" | "connecting" | "online" | "offline"
export type EngineTarget = { url: string; headers?: Record<string, string> }

type TauriGlobal = { core?: { invoke: (cmd: string) => Promise<unknown> } }
type ShellEngineStatus = { url?: string; error?: string; password?: string }

// How long to wait for the shell to report a ready embedded engine before giving up.
const engineReadyTimeoutMs = 45_000
const engineReadyPollMs = 300
// The embedded engine is always addressed with this fixed username; only the password varies.
const engineUsername = "opencode"

export async function resolveEngine(): Promise<EngineTarget> {
  const tauri = (globalThis as { __TAURI__?: TauriGlobal }).__TAURI__
  if (tauri?.core) return waitForShellEngine(tauri.core)
  return {
    url: import.meta.env.VITE_ENGINE_URL ?? "http://127.0.0.1:4096",
    headers: basicAuth(import.meta.env.VITE_ENGINE_USERNAME, import.meta.env.VITE_ENGINE_PASSWORD),
  }
}

async function waitForShellEngine(core: NonNullable<TauriGlobal["core"]>): Promise<EngineTarget> {
  const deadline = Date.now() + engineReadyTimeoutMs
  for (;;) {
    const status = (await core.invoke("engine_status")) as ShellEngineStatus
    if (status.url) return { url: status.url, headers: basicAuth(engineUsername, status.password) }
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
