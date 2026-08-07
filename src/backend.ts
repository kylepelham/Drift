import { isRemoteRuntime } from "./runtime"
import { shellInvoke, type ShellInvoke } from "./shell"

export function backendRoute(desktop: boolean, remote: boolean) {
  return desktop ? "tauri" : remote ? "rpc" : "browser"
}

export function backendInvoke(): ShellInvoke | undefined {
  const desktop = shellInvoke()
  const route = backendRoute(!!desktop, isRemoteRuntime())
  if (route === "tauri") return desktop
  if (route === "browser") return undefined
  return async <T>(command: string, args: Record<string, unknown> = {}) => {
    const response = await fetch("/api/invoke", {
      method: "POST",
      credentials: "same-origin",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ command, args }),
    })
    const value = (await response.json().catch(() => null)) as T | { error?: string } | null
    if (!response.ok) {
      const message = value && typeof value === "object" && "error" in value ? value.error : undefined
      throw new Error(message || `Backend request failed (${response.status})`)
    }
    return value as T
  }
}
