import { backendInvoke } from "../backend"
import { isRemoteRuntime } from "../runtime"
import { shellEvents } from "../shell"

export type MirrorThemeName =
  | "drift-dark"
  | "drift-graphite"
  | "drift-midnight"
  | "drift-slate"
  | "drift-forest"
  | "drift-aubergine"
  | "drift-light"
  | "drift-paper"
  | "drift-custom"

export type MirrorTheme = {
  name: MirrorThemeName
  custom: { background: string; surface: string; text: string; accent: string }
  uiFont: string
  codeFont: string
  customCss: string
}

export type MirrorSelection = { workspaceId: string | null; sessionId: string | null }
export type UiMirrorSnapshot = {
  schema: 1
  revision: number
  theme: MirrorTheme
  selection: MirrorSelection
}

type MirrorApplier = {
  theme: (theme: MirrorTheme) => void
  selection: (selection: MirrorSelection) => void
}

const defaults: MirrorTheme = {
  name: "drift-dark",
  custom: { background: "#111318", surface: "#1b1e25", text: "#e8eaf0", accent: "#a78bfa" },
  uiFont: "",
  codeFont: "",
  customCss: "",
}

const clientId = globalThis.crypto?.randomUUID?.() ?? `client-${Date.now()}-${Math.random()}`
let current: UiMirrorSnapshot | undefined
let applier: MirrorApplier | undefined
let queued: { theme?: MirrorTheme; selection?: MirrorSelection } | undefined
let retry: {
  patch: { theme?: MirrorTheme; selection?: MirrorSelection }
  mutation: { clientId: string; mutationId: string; theme?: MirrorTheme; selection?: MirrorSelection }
} | undefined
let publishing = false
let retryTimer: ReturnType<typeof setTimeout> | undefined
let liveStarted = false
let liveError = ""
const liveErrorListeners = new Set<(error: string) => void>()

function stored<T>(key: string, fallback: T): T {
  try {
    const value = localStorage.getItem(key)
    return value ? (JSON.parse(value) as T) : fallback
  } catch {
    return fallback
  }
}

export function localMirrorSnapshot(): UiMirrorSnapshot {
  return {
    schema: 1,
    revision: 0,
    theme: {
      name: stored("drift.theme", defaults.name),
      custom: stored("drift.theme.custom", defaults.custom),
      uiFont: stored("drift.theme.uiFont", ""),
      codeFont: stored("drift.theme.codeFont", ""),
      customCss: stored("drift.theme.customCss", ""),
    },
    selection: {
      workspaceId: stored<string | null>("drift.workspace", null),
      sessionId: stored<string | null>("drift.session", null),
    },
  }
}

function cache(snapshot: UiMirrorSnapshot) {
  const values: [string, unknown][] = [
    ["drift.theme", snapshot.theme.name],
    ["drift.theme.custom", snapshot.theme.custom],
    ["drift.theme.uiFont", snapshot.theme.uiFont],
    ["drift.theme.codeFont", snapshot.theme.codeFont],
    ["drift.theme.customCss", snapshot.theme.customCss],
    ["drift.workspace", snapshot.selection.workspaceId],
    ["drift.session", snapshot.selection.sessionId],
  ]
  for (const [key, value] of values) localStorage.setItem(key, JSON.stringify(value))
}

export function acceptMirrorSnapshot(snapshot: UiMirrorSnapshot, force = false) {
  if (!force && current && !shouldAcceptRevision(current.revision, snapshot.revision)) return false
  current = snapshot
  cache(snapshot)
  applier?.theme(snapshot.theme)
  applier?.selection(snapshot.selection)
  return true
}

export function shouldAcceptRevision(currentRevision: number, incomingRevision: number) {
  return incomingRevision > currentRevision
}

export function mirrorBootstrapCommand(remote: boolean) {
  return remote ? "ui_state_snapshot" : "ui_state_initialize"
}

export function shellTimeoutBootstrapCommand(remote: boolean) {
  return remote ? "shell_timeout_snapshot" : "shell_timeout_initialize"
}

export function registerMirrorApplier(next: MirrorApplier) {
  applier = next
  if (current) {
    next.theme(current.theme)
    next.selection(current.selection)
  }
}

export async function bootstrapMirror() {
  const invoke = backendInvoke()
  if (!invoke) throw new Error("The Drift host bridge is unavailable.")
  const remote = isRemoteRuntime()
  const snapshot = remote
    ? await invoke<UiMirrorSnapshot>(mirrorBootstrapCommand(true))
    : await invoke<UiMirrorSnapshot>(mirrorBootstrapCommand(false), { snapshot: localMirrorSnapshot() })
  acceptMirrorSnapshot(snapshot, true)

  const localTimeout = stored<number | null>("drift.shell.timeout", null)
  const policy = remote
    ? await invoke<{ timeoutMs: number | null }>(shellTimeoutBootstrapCommand(true))
    : await invoke<{ timeoutMs: number | null }>(shellTimeoutBootstrapCommand(false), { policy: { timeoutMs: localTimeout } })
  localStorage.setItem("drift.shell.timeout", JSON.stringify(policy.timeoutMs))
}

export function publishMirrorTheme(theme: MirrorTheme) {
  if (current) current = { ...current, theme }
  queueMutation({ theme })
}

export function publishMirrorSelection(selection: MirrorSelection) {
  if (current) current = { ...current, selection }
  queueMutation({ selection })
}

function queueMutation(patch: { theme?: MirrorTheme; selection?: MirrorSelection }) {
  if (!current) return
  queued = { ...queued, ...patch }
  if (!publishing) void flushMutations()
}

async function flushMutations() {
  const invoke = backendInvoke()
  if (!invoke) return
  publishing = true
  let failed = false
  try {
    while (retry || queued) {
      if (!retry) {
        const patch = queued!
        queued = undefined
        retry = {
          patch,
          mutation: {
            clientId,
            mutationId: globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random()}`,
            ...patch,
          },
        }
      }
      try {
        const snapshot = await invoke<UiMirrorSnapshot>("ui_state_update", { mutation: retry.mutation })
        retry = undefined
        acceptMirrorSnapshot(snapshot)
        setLiveError("")
      } catch (cause) {
        failed = true
        setLiveError(cause instanceof Error ? cause.message : String(cause))
        break
      }
    }
  } finally {
    publishing = false
    if (failed) {
      retryTimer = setTimeout(() => {
        retryTimer = undefined
        void flushMutations()
      }, 1500)
    } else if ((retry || queued) && !retryTimer) {
      void flushMutations()
    }
  }
}

export function startMirrorEvents() {
  if (liveStarted) return
  liveStarted = true
  if (!isRemoteRuntime()) {
    void shellEvents()?.listen<UiMirrorSnapshot>("ui-state-changed", (event) => acceptMirrorSnapshot(event.payload))
    return
  }
  connectRemoteEvents()
}

function connectRemoteEvents() {
  const source = new EventSource("/api/ui-state/events", { withCredentials: true })
  source.onopen = () => setLiveError("")
  source.onmessage = (event) => {
    try {
      acceptMirrorSnapshot(JSON.parse(event.data) as UiMirrorSnapshot)
      setLiveError("")
    } catch {
      setLiveError("The host sent invalid mirror state.")
    }
  }
  source.onerror = () => {
    source.close()
    setLiveError("Connection to the Drift host was lost. Reconnecting...")
    setTimeout(async () => {
      try {
        const snapshot = await backendInvoke()!<UiMirrorSnapshot>("ui_state_snapshot")
        acceptMirrorSnapshot(snapshot)
        connectRemoteEvents()
      } catch (cause) {
        setLiveError(cause instanceof Error ? cause.message : String(cause))
        setTimeout(connectRemoteEvents, 3000)
      }
    }, 1000)
  }
}

function setLiveError(error: string) {
  liveError = error
  for (const listener of liveErrorListeners) listener(error)
}

export function mirrorLiveError() {
  return liveError
}

export function listenMirrorLiveError(listener: (error: string) => void) {
  liveErrorListeners.add(listener)
  listener(liveError)
  return () => liveErrorListeners.delete(listener)
}

export function currentMirrorSnapshot() {
  return current
}
