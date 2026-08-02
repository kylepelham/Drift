import { createSignal } from "solid-js"
import { shellInvoke } from "../shell"

export type RemoteAccessStatus = {
  enabled: boolean
  listening: boolean
  port: number
  discoveryPort: number
  listeningAddress?: string
  urls: string[]
  connectionUrls: string[]
  error?: string
}

export function remoteStatusTone(status: RemoteAccessStatus | null) {
  if (!status) return "idle"
  if (status.error) return "error"
  return status.enabled && status.listening ? "online" : "offline"
}

const [status, setStatus] = createSignal<RemoteAccessStatus | null>(null)
const [busy, setBusy] = createSignal(false)
const [error, setError] = createSignal("")

export { status as remoteAccessStatus, busy as remoteAccessBusy, error as remoteAccessError }

async function call(command: string) {
  const invoke = shellInvoke()
  if (!invoke) return
  setBusy(true)
  setError("")
  try {
    setStatus(await invoke<RemoteAccessStatus>(command))
  } catch (cause) {
    setError(cause instanceof Error ? cause.message : String(cause))
  } finally {
    setBusy(false)
  }
}

export function refreshRemoteAccess() {
  return call("remote_access_status")
}

export function setRemoteAccess(enabled: boolean) {
  return call(enabled ? "remote_access_enable" : "remote_access_disable")
}

export function rotateRemoteAccessToken() {
  return call("remote_access_rotate_token")
}
