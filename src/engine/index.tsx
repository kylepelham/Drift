import { createOpencodeClient, type OpencodeClient } from "@opencode-ai/sdk/client"
import { createContext, createEffect, onCleanup, useContext, type ParentProps } from "solid-js"
import { produce } from "solid-js/store"
import { shellEvents, shellInvoke } from "../shell"
import { t } from "../state/i18n"
import { selectedSession } from "../state/selection"
import { clearPermissionAttentionFor } from "../state/permission-attention"
import { clearRecoverableInterruption } from "../state/recovery"
import { reportShellTimeoutError, shellTimeoutMs } from "../state/prefs"
import { applyProviderCatalog, seedProviderCatalog } from "../state/provider-cache"
import { createActions, type EngineActions } from "./actions"
import {
  configureShellTimeout,
  inspectShellEngine,
  resolveEngine,
  restartShellEngine,
  sleep,
  type EngineTarget,
  type ShellEngineInspection,
} from "./connection"
import { applySessionSnapshot, applyStatusSnapshot, reduce } from "./events"
import { streamEvents } from "./sse"
import { seedBench } from "./bench"
import {
  captureRevisions,
  compareMessages,
  createEngineState,
  interruptStaleTools,
  mergeTranscriptSnapshot,
  sessionSnapshotLimit,
  type EngineState,
} from "./store"

export type Engine = {
  state: EngineState
  actions: EngineActions
  setDirectory: (path: string | null) => void
  restartEngine: () => Promise<boolean>
  refreshRuntimeMetadata: () => Promise<void>
}

const EngineContext = createContext<Engine>()

// Reconnect transcript refreshes run in small batches so they cannot starve the visible session.
const transcriptRefreshBatch = 3
const engineInspectionTimeoutMs = 5_000

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
  // Cold engine startup can take seconds; the last known catalog keeps the model picker populated
  // (and the saved model preference resolvable) until the first hydrate delivers fresh providers.
  seedProviderCatalog(state, set)
  let base: EngineTarget | undefined
  let client: OpencodeClient | undefined
  let pumpAbort: AbortController | undefined
  let directory: string | null = null
  let disposed = false
  let engineEpoch = 0
  let restartRequest: Promise<boolean> | undefined
  let unlistenEngineExit: (() => void) | undefined
  let unlistenSkillConfig: (() => void) | undefined
  let unlistenMcpConfig: (() => void) | undefined
  let runtimeMetadataEpoch = 0
  let runtimeInspection: Promise<ShellEngineInspection | undefined> | undefined
  let timeoutSync = Promise.resolve()

  const requireClient = () => {
    if (!client) throw new Error("engine offline")
    return client
  }
  const actions = createActions(requireClient, state, set, () => base)

  function syncShellTimeout(target = base) {
    if (!target) return
    const timeout = shellTimeoutMs()
    timeoutSync = timeoutSync
      .then(() => {
        if (disposed || base !== target || shellTimeoutMs() !== timeout) return
        return configureShellTimeout(target, timeout)
      })
      .then(() => reportShellTimeoutError(""))
      .catch((cause) => reportShellTimeoutError(cause instanceof Error ? cause.message : String(cause)))
  }

  createEffect(() => {
    shellTimeoutMs()
    syncShellTimeout()
  })

  async function hydrate() {
    const bootDirectory = directory ?? ""
    const api = requireClient()
    const providerEpoch = state.providerSnapshotEpoch + 1
    const metadataEpoch = runtimeMetadataEpoch + 1
    runtimeMetadataEpoch = metadataEpoch
    set("providerSnapshotEpoch", providerEpoch)
    // A reconnect flap can overlap two hydrates against the same client and directory; the epoch
    // (bumped by every server.connected) keeps the stale one from purging what the fresh one wrote.
    const epoch = state.sessionSnapshotEpoch
    const current = () =>
      client === api && directory === bootDirectory && state.sessionSnapshotEpoch === epoch
    try {
      const stale = Object.keys(state.loaded)
      const captured = captureRevisions(state)
      const [sessions, [statuses, providers, agents, commands, config]] = await Promise.all([
        api.session.list(),
        Promise.all([api.session.status(), api.provider.list(), api.app.agents(), api.command.list(), api.config.get()]),
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
      if (complete) set("sessionSnapshotDirectory", bootDirectory)
      applyStatusSnapshot(set, { sessions: list, statuses: statuses.data ?? {}, captured })
      for (const [sessionID, status] of Object.entries(statuses.data ?? {}))
        if (status.type !== "idle") clearRecoverableInterruption(sessionID, true)
      // Pending permissions/questions exist only as events; a bounded SSE buffer can drop the ask
      // frame under load, which would strand the session busy forever without this refetch.
      if (bootDirectory) void actions.refreshPermissions([bootDirectory])
      if (state.providerSnapshotEpoch === providerEpoch) applyProviderCatalog(set, providers.data)
      set("agents", agents.data ?? [])
      if (runtimeMetadataEpoch === metadataEpoch) {
        set("commands", commands.data ?? [])
        if (config.data !== undefined) syncSkillWatchPaths(bootDirectory, config.data)
      }
      // The visible session refreshes first so a reconnect never leaves the open transcript
      // waiting behind bulk refetches; the rest trickle in small batches to avoid saturating
      // the handful of HTTP connections a remote browser gives the proxy.
      const selected = selectedSession()
      const ordered = [...stale].sort((a, b) => Number(b === selected) - Number(a === selected))
      for (let index = 0; index < ordered.length; index += transcriptRefreshBatch) {
        const transcripts = await Promise.all(
          ordered
            .slice(index, index + transcriptRefreshBatch)
            .map(async (id) => [id, await api.session.messages({ path: { id }, query: { limit: 100 } })] as const),
        )
        if (!current()) return
        for (const [id, result] of transcripts) {
          if (!result.data) continue
          // Reconciliation or a deletion event removed the session; do not resurrect its transcript.
          if (!state.sessions[id]) continue
          const entries = interruptStaleTools(
            [...result.data].sort(compareMessages),
            state.liveTools,
            t("drift.message.interrupted"),
          )
          set("transcripts", id, mergeTranscriptSnapshot(state.transcripts[id], entries, id, captured, state.revisions))
          set("cursors", id, result.response?.headers?.get("x-next-cursor") ?? null)
          const latest = [...entries].reverse().find((entry) => entry.info.role === "assistant")?.info
          if (latest?.role === "assistant" && latest.time.completed && !latest.error)
            clearRecoverableInterruption(id, true)
        }
      }
      if (!state.version && base) {
        const target = base
        const health = await fetchEngineVersion(target)
        if (!current() || base !== target) return
        if (health?.version) set("version", health.version)
      }
    } finally {
      // A hydrate that bailed on a stale epoch wrote nothing, so it must not report readiness.
      if (current()) set("bootstrappedDirectory", bootDirectory)
    }
  }

  function syncSkillWatchPaths(bootDirectory: string, config: unknown) {
    const invoke = shellInvoke()
    if (!invoke || !bootDirectory) return
    const paths = (config as { skills?: { paths?: unknown } } | undefined)?.skills?.paths
    void invoke("watcher_set_skill_paths", {
      directory: bootDirectory,
      paths: Array.isArray(paths) ? paths.filter((path): path is string => typeof path === "string") : [],
    }).catch(() => undefined)
  }

  async function refreshRuntimeMetadata() {
    const api = client
    const bootDirectory = directory ?? ""
    if (!api || !bootDirectory || disposed) return
    const metadataEpoch = runtimeMetadataEpoch + 1
    runtimeMetadataEpoch = metadataEpoch
    let commands: NonNullable<Awaited<ReturnType<typeof api.command.list>>["data"]> | undefined
    let config: NonNullable<Awaited<ReturnType<typeof api.config.get>>["data"]> | undefined
    const load = async () => {
      for (let attempt = 0; attempt < 3; attempt += 1) {
        const [nextCommands, nextConfig] = await Promise.all([
          api.command.list().catch(() => undefined),
          api.config.get().catch(() => undefined),
        ])
        if (nextCommands?.data !== undefined) commands = nextCommands.data
        if (nextConfig?.data !== undefined) config = nextConfig.data
        if (commands && config) break
        await sleep(150 * (attempt + 1))
      }
    }
    await load()
    if (!commands || !config) {
      await sleep(1000)
      if (disposed || client !== api || directory !== bootDirectory || runtimeMetadataEpoch !== metadataEpoch) return
      await load()
    }
    if (disposed || client !== api || directory !== bootDirectory || runtimeMetadataEpoch !== metadataEpoch) return
    if (commands) set("commands", commands)
    if (config) syncSkillWatchPaths(bootDirectory, config)
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
    // The timeout cannot cancel the backend call, so retries share the one still in flight rather
    // than stacking a new invoke per attempt.
    runtimeInspection ??= inspectShellEngine()
      .catch(() => undefined)
      .finally(() => {
        runtimeInspection = undefined
      })
    const status = await Promise.race([runtimeInspection, sleep(engineInspectionTimeoutMs).then(() => undefined)])
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
      let streamError: unknown
      try {
        await streamEvents(target, signal, (event, eventDirectory) => {
          if (event.type === "server.connected") {
            set("sessionSnapshotDirectory", "")
            set("sessionSnapshotAll", false)
            set("sessionSnapshotEpoch", state.sessionSnapshotEpoch + 1)
            set("connection", "online")
            void hydrate().catch(() => undefined)
            return
          }
          try {
            reduce(set, event, eventDirectory, state)
          } catch (cause) {
            // One malformed or locally unpersistable event must not kill all future updates.
            console.error("Drift could not reduce engine event", {
              type: event.type,
              directory: eventDirectory,
              cause,
            })
          }
        })
      } catch (cause) {
        streamError = cause
      }
      if (signal.aborted) return
      set("connection", "offline")
      if (streamError) console.warn("Drift engine event stream disconnected", streamError)
      if (await inspectRuntimeEngine()) return
      if (signal.aborted) return
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
    const controller = new AbortController()
    pumpAbort = controller
    void pump(controller.signal)
      .catch((cause) => {
        if (!controller.signal.aborted) console.error("Drift engine event pump stopped unexpectedly", cause)
      })
      .finally(() => {
        if (pumpAbort !== controller) return
        pumpAbort = undefined
        if (!disposed && !controller.signal.aborted && base && directory) startPump(directory)
      })
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
        syncShellTimeout(target)
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
      syncShellTimeout(target)
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
    void Promise.all([
      events.listen("engine-exited", () => void inspectRuntimeEngine()).then((unlisten) => {
        if (disposed) unlisten()
        else unlistenEngineExit = unlisten
      }),
      events.listen("skill-config-changed", () => void refreshRuntimeMetadata().catch(() => undefined)).then((unlisten) => {
        if (disposed) unlisten()
        else unlistenSkillConfig = unlisten
      }),
      events.listen("mcp-config-changed", () => void refreshRuntimeMetadata().catch(() => undefined)).then((unlisten) => {
        if (disposed) unlisten()
        else unlistenMcpConfig = unlisten
      }),
    ]).catch(() => undefined)
  onCleanup(() => {
    disposed = true
    pumpAbort?.abort()
    unlistenEngineExit?.()
    unlistenSkillConfig?.()
    unlistenMcpConfig?.()
    clearPermissionAttentionFor(Object.values(state.permissions).flat())
  })

  return (
    <EngineContext.Provider value={{ state, actions, setDirectory, restartEngine, refreshRuntimeMetadata }}>
      {props.children}
    </EngineContext.Provider>
  )
}
