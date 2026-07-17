export type Connection = "connecting" | "online" | "offline"
export type EngineTarget = { url: string; headers?: Record<string, string> }

type TauriGlobal = { core?: { invoke: (cmd: string) => Promise<string> } }

export async function resolveEngine(): Promise<EngineTarget> {
  const tauri = (globalThis as { __TAURI__?: TauriGlobal }).__TAURI__
  if (tauri?.core) return { url: await waitForShellEngine(tauri.core) }
  return {
    url: import.meta.env.VITE_ENGINE_URL ?? "http://127.0.0.1:4096",
    headers: basicAuth(import.meta.env.VITE_ENGINE_USERNAME, import.meta.env.VITE_ENGINE_PASSWORD),
  }
}

async function waitForShellEngine(core: NonNullable<TauriGlobal["core"]>): Promise<string> {
  for (;;) {
    const url = await core.invoke("engine_url").catch(() => null)
    if (url) return url
    await sleep(300)
  }
}

function basicAuth(username?: string, password?: string) {
  if (!password) return undefined
  return { Authorization: `Basic ${btoa(`${username ?? "opencode"}:${password}`)}` }
}

export function sleep(ms: number) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}
