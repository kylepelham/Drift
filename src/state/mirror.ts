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
  workspaceOrder: string[]
}

type MirrorApplier = {
  theme: (theme: MirrorTheme) => void
  selection: (selection: MirrorSelection) => void
  order: (ids: string[]) => void
}

type MirrorPatch = { theme?: MirrorTheme; selection?: MirrorSelection; workspaceOrder?: string[] }

const defaults: MirrorTheme = {
  name: "drift-dark",
  custom: { background: "#111318", surface: "#1b1e25", text: "#e8eaf0", accent: "#a78bfa" },
  uiFont: "",
  codeFont: "",
  customCss: "",
}
const themeNames = new Set<MirrorThemeName>([
  "drift-dark",
  "drift-graphite",
  "drift-midnight",
  "drift-slate",
  "drift-forest",
  "drift-aubergine",
  "drift-light",
  "drift-paper",
  "drift-custom",
])
const hexColor = /^#[\da-f]{6}$/i

const clientId = globalThis.crypto?.randomUUID?.() ?? `client-${Date.now()}-${Math.random()}`
let current: UiMirrorSnapshot | undefined
let applier: MirrorApplier | undefined
let queued: MirrorPatch | undefined
let retry: {
  patch: MirrorPatch
  mutation: MirrorPatch & { clientId: string; mutationId: string }
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

function boundedString(value: unknown, fallback: string, max: number) {
  return typeof value === "string" ? [...value].slice(0, max).join("") : fallback
}

function identifier(value: unknown) {
  return typeof value === "string" && [...value].length > 0 && [...value].length <= 256 && ![...value].some((char) => /\p{Cc}/u.test(char))
    ? value
    : null
}

function customTheme(value: unknown): MirrorTheme["custom"] {
  const source = value && typeof value === "object" ? value as Record<string, unknown> : {}
  return {
    background: typeof source.background === "string" && hexColor.test(source.background) ? source.background : defaults.custom.background,
    surface: typeof source.surface === "string" && hexColor.test(source.surface) ? source.surface : defaults.custom.surface,
    text: typeof source.text === "string" && hexColor.test(source.text) ? source.text : defaults.custom.text,
    accent: typeof source.accent === "string" && hexColor.test(source.accent) ? source.accent : defaults.custom.accent,
  }
}

export function localMirrorSnapshot(): UiMirrorSnapshot {
  const name = stored<unknown>("drift.theme", defaults.name)
  const workspaceId = identifier(stored<unknown>("drift.workspace", null))
  const sessionId = workspaceId ? identifier(stored<unknown>("drift.session", null)) : null
  const order = stored<unknown>("drift.workspace.order", [])
  const workspaceOrder = Array.isArray(order)
    ? [...new Set(order.map(identifier).filter((id): id is string => !!id))].slice(0, 500)
    : []
  return {
    schema: 1,
    revision: 0,
    theme: {
      name: typeof name === "string" && themeNames.has(name as MirrorThemeName) ? name as MirrorThemeName : defaults.name,
      custom: customTheme(stored<unknown>("drift.theme.custom", defaults.custom)),
      uiFont: boundedString(stored<unknown>("drift.theme.uiFont", ""), "", 256),
      codeFont: boundedString(stored<unknown>("drift.theme.codeFont", ""), "", 256),
      customCss: boundedString(stored<unknown>("drift.theme.customCss", ""), "", 20_000),
    },
    selection: { workspaceId, sessionId },
    workspaceOrder,
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
    ["drift.workspace.order", snapshot.workspaceOrder],
  ]
  for (const [key, value] of values) cacheValue(key, value)
}

function cacheValue(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value))
  } catch {}
}

export function acceptMirrorSnapshot(snapshot: UiMirrorSnapshot, force = false) {
  if (!force && current && !shouldAcceptRevision(current.revision, snapshot.revision)) return false
  current = snapshot
  cache(snapshot)
  applier?.theme(snapshot.theme)
  applier?.order(snapshot.workspaceOrder)
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
    next.order(current.workspaceOrder)
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

  const savedTimeout = stored<unknown>("drift.shell.timeout", null)
  const localTimeout = typeof savedTimeout === "number" && Number.isInteger(savedTimeout) && savedTimeout >= 60_000 && savedTimeout <= 86_400_000
    ? savedTimeout
    : null
  const policy = remote
    ? await invoke<{ timeoutMs: number | null }>(shellTimeoutBootstrapCommand(true))
    : await invoke<{ timeoutMs: number | null }>(shellTimeoutBootstrapCommand(false), { policy: { timeoutMs: localTimeout } })
  cacheValue("drift.shell.timeout", policy.timeoutMs)
}

export function publishMirrorTheme(theme: MirrorTheme) {
  if (current) current = { ...current, theme }
  queueMutation({ theme })
}

export function publishMirrorSelection(selection: MirrorSelection) {
  if (current) current = { ...current, selection }
  queueMutation({ selection })
}

export function publishMirrorWorkspaceOrder(ids: string[]) {
  if (current && current.workspaceOrder.join("\u0000") === ids.join("\u0000")) return
  if (current) current = { ...current, workspaceOrder: ids }
  queueMutation({ workspaceOrder: ids })
}

function queueMutation(patch: MirrorPatch) {
  if (!current) return
  queued = { ...queued, ...patch }
  if (!publishing) void flushMutations()
}

async function flushMutations() {
  const invoke = backendInvoke()
  if (!invoke) return
  if (retryTimer) {
    clearTimeout(retryTimer)
    retryTimer = undefined
  }
  if (retry && queued) {
    queued = { ...retry.patch, ...queued }
    retry = undefined
  }
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
        const message = cause instanceof Error ? cause.message : String(cause)
        if (message === "selected workspace does not exist") {
          retry = undefined
          setLiveError("")
          continue
        }
        failed = true
        setLiveError(message)
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
