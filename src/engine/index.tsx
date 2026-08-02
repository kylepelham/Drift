import { createOpencodeClient, type OpencodeClient } from "@opencode-ai/sdk/client"
import { createContext, onCleanup, useContext, type ParentProps } from "solid-js"
import { produce } from "solid-js/store"
import { shellEvents } from "../shell"
import { t } from "../state/i18n"
import { clearPermissionAttentionFor } from "../state/permission-attention"
import { clearRecoverableInterruption } from "../state/recovery"
import { createActions, type EngineActions } from "./actions"
import { inspectShellEngine, resolveEngine, restartShellEngine, sleep, type EngineTarget } from "./connection"
import { applySessionSnapshot, applyStatusSnapshot, reduce } from "./events"
import { streamEvents } from "./sse"
import { seedBench } from "./bench"
import {
  captureRevisions,
  createEngineState,
  interruptStaleTools,
  mergeTranscriptSnapshot,
  sessionSnapshotLimit,
  type EngineState,
  type ProviderInfo,
} from "./store"

export type Engine = {
  state: EngineState
  actions: EngineActions
  setDirectory: (path: string | null) => void
  restartEngine: () => Promise<boolean>
}

const EngineContext = createContext<Engine>()

/** Reads the engine version, or null if the engine is unreachable or does not answer with it. */
function fetchEngineVersion(target: EngineTarget) {
  return fetch(`${target.url}/global/health`, { headers: target.headers })
    .then((response) => (response.ok ? (response.json() as Promise<{ version?: string }>) : null))
    .catch(() => null)
}

export function useEngine() {
  const engine = useContext(EngineContext)
  if (!engine) throw new Error("useEngine outside EngineProvider")
  return engine
}

export function EngineProvider(props: ParentProps) {
  const [state, set] = createEngineState()
  let base: EngineTarget | undefined
  let client: OpencodeClient | undefined
  let pumpAbort: AbortController | undefined
  let directory: string | null = null
  let disposed = false
  let engineEpoch = 0
  let restartRequest: Promise<boolean> | undefined
  let unlistenEngineExit: (() => void) | undefined

  const requireClient = () => {
    if (!client) throw new Error("engine offline")
    return client
  }
  const actions = createActions(requireClient, state, set, () => base)

  async function hydrate() {
    const bootDirectory = directory ?? ""
    const api = requireClient()
    const current = () => client === api && directory === bootDirectory
    try {
      const stale = Object.keys(state.loaded)
      const captured = captureRevisions(state)
      const [sessions, [statuses, providers, agents, commands]] = await Promise.all([
        api.session.list(),
        Promise.all([api.session.status(), api.provider.list(), api.app.agents(), api.command.list()]),
      ])
      if (!current()) return
      const list = sessions.data ?? []
      // Only a successful, untruncated listing is authoritative for the workspace.
      const complete = sessions.error === undefined && sessions.data !== undefined && list.length < sessionSnapshotLimit
      applySessionSnapshot(set, {
        sessions: list,
        captured,
        ...(complete && bootDirectory ? { scope: { directory: bootDirectory } } : {}),
      })
      applyStatusSnapshot(set, { sessions: list, statuses: statuses.data ?? {}, captured })
      for (const [sessionID, status] of Object.entries(statuses.data ?? {}))
        if (status.type !== "idle") clearRecoverableInterruption(sessionID, true)
      set("providers", (providers.data?.all ?? []) as unknown as ProviderInfo[])
      set("connected", providers.data?.connected ?? [])
      set("defaultModels", providers.data?.default ?? {})
      set("agents", agents.data ?? [])
      set("commands", commands.data ?? [])
      const transcripts = await Promise.all(
        stale.map(async (id) => [id, await api.session.messages({ path: { id }, query: { limit: 100 } })] as const),
      )
      if (!current()) return
      for (const [id, result] of transcripts) {
        if (!result.data) continue
        // Reconciliation or a deletion event removed the session; do not resurrect its transcript.
        if (!state.sessions[id]) continue
        const entries = interruptStaleTools(
          [...result.data].sort((a, b) => a.info.id.localeCompare(b.info.id)),
          state.liveTools,
          t("drift.message.interrupted"),
        )
        set("transcripts", id, mergeTranscriptSnapshot(state.transcripts[id], entries, id, captured, state.revisions))
        set("cursors", id, result.response?.headers?.get("x-next-cursor") ?? null)
        const latest = [...entries].reverse().find((entry) => entry.info.role === "assistant")?.info
        if (latest?.role === "assistant" && latest.time.completed && !latest.error)
          clearRecoverableInterruption(id, true)
      }
      if (!state.version && base) {
        const target = base
        const health = await fetchEngineVersion(target)
        if (!current() || base !== target) return
        if (health?.version) set("version", health.version)
      }
    } finally {
      if (client === api && directory === bootDirectory) set("bootstrappedDirectory", bootDirectory)
    }
  }

  function sameTarget(left: EngineTarget | undefined, right: EngineTarget) {
    return left?.url === right.url && JSON.stringify(left.headers ?? {}) === JSON.stringify(right.headers ?? {})
  }

  function recordEngineFailure(message: string) {
    engineEpoch += 1
    stopPump()
    base = undefined
    set(
      produce((draft) => {
        draft.engineError = message
        draft.engineRestarting = false
        draft.connection = directory ? "offline" : "idle"
        draft.liveTools = {}
      }),
    )
  }

  async function inspectRuntimeEngine() {
    const epoch = engineEpoch
    const status = await inspectShellEngine().catch(() => undefined)
    if (!status || disposed || epoch !== engineEpoch) return false
    if (status.error) {
      recordEngineFailure(status.error)
      return true
    }
    if (status.target && !sameTarget(base, status.target)) {
      base = status.target
      set("liveTools", {})
      if (directory) client = createOpencodeClient({ baseUrl: base.url, headers: base.headers, directory })
    }
    return false
  }

  async function pump(signal: AbortSignal) {
    while (!signal.aborted) {
      const target = base
      if (!target) return
      try {
        await streamEvents(target, signal, (event, eventDirectory) => {
          if (event.type === "server.connected") {
            set("connection", "online")
            void hydrate().catch(() => undefined)
            return
          }
          reduce(set, event, eventDirectory, state)
        })
      } catch {}
      if (signal.aborted) return
      if (await inspectRuntimeEngine()) return
      if (signal.aborted) return
      set("connection", "offline")
      await sleep(1500)
      if (signal.aborted) return
      set("connection", "connecting")
    }
  }

  // Session-keyed state and the global event stream survive directory switches. Only the
  // SDK client is re-pointed so workspace changes don't reconnect or wipe transcripts.
  function stopPump() {
    pumpAbort?.abort()
    pumpAbort = undefined
    client = undefined
  }

  function startPump(path: string) {
    if (!base || disposed || pumpAbort) return
    client = createOpencodeClient({ baseUrl: base.url, headers: base.headers, directory: path })
    set(
      produce((draft) => {
        draft.directory = path
        draft.connection = "connecting"
      }),
    )
    if (import.meta.env.DEV) seedBench(set, path)
    pumpAbort = new AbortController()
    void pump(pumpAbort.signal)
  }

  function restartEngine() {
    if (restartRequest) return restartRequest
    engineEpoch += 1
    const epoch = engineEpoch
    stopPump()
    base = undefined
    set(
      produce((draft) => {
        draft.engineRestarting = true
        draft.connection = directory ? "connecting" : "idle"
        draft.liveTools = {}
      }),
    )
    let request!: Promise<boolean>
    request = restartShellEngine()
      .then((target) => {
        if (disposed || epoch !== engineEpoch) return false
        base = target
        set(
          produce((draft) => {
            draft.engineError = ""
            draft.engineRestarting = false
            draft.startupError = ""
          }),
        )
        if (directory) startPump(directory)
        return true
      })
      .catch((error: unknown) => {
        if (!disposed && epoch === engineEpoch) recordEngineFailure(error instanceof Error ? error.message : String(error))
        return false
      })
      .finally(() => {
        if (restartRequest === request) restartRequest = undefined
      })
    restartRequest = request
    return request
  }

  function setDirectory(path: string | null) {
    // Already on this directory with nothing left to do: either a client exists, or the target is
    // null and there is nothing to connect to. Without the client check a repeated call before the
    // first connection completed would be dropped and never start the pump.
    if (path === directory && (client || !path)) return
    const prev = directory
    directory = path
    if (!base || disposed) return
    if (!path) {
      stopPump()
      set(
        produce((draft) => {
          draft.directory = ""
          draft.connection = "idle"
        }),
      )
      return
    }
    // With no stream running, or no previous directory, there is nothing to reuse - start fresh.
    if (!pumpAbort || !prev) {
      stopPump()
      startPump(path)
      return
    }
    // Otherwise the event stream is global and already running, so switching workspaces only means
    // pointing the client at the new directory. Restarting the stream here would drop events.
    client = createOpencodeClient({ baseUrl: base.url, headers: base.headers, directory: path })
    set("directory", path)
    if (state.connection === "online") void hydrate().catch(() => undefined)
  }

  const startupEpoch = engineEpoch
  void resolveEngine()
    .then(async (target) => {
      if (disposed || startupEpoch !== engineEpoch) return
      base = target
      set("engineError", "")
      const health = await fetchEngineVersion(target)
      if (disposed || startupEpoch !== engineEpoch) return
      if (health?.version) set("version", health.version)
      if (directory) startPump(directory)
    })
    .catch((error: unknown) => {
      if (disposed || startupEpoch !== engineEpoch) return
      const message = error instanceof Error ? error.message : String(error)
      set("startupError", message)
      set("engineError", message)
      if (directory) set("connection", "offline")
    })
  const events = shellEvents()
  if (events)
    void events
      .listen("engine-exited", () => void inspectRuntimeEngine())
      .then((unlisten) => {
        if (disposed) unlisten()
        else unlistenEngineExit = unlisten
      })
      .catch(() => undefined)
  onCleanup(() => {
    disposed = true
    pumpAbort?.abort()
    unlistenEngineExit?.()
    clearPermissionAttentionFor(Object.values(state.permissions).flat())
  })

  return <EngineContext.Provider value={{ state, actions, setDirectory, restartEngine }}>{props.children}</EngineContext.Provider>
}
