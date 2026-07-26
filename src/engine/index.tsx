import { createOpencodeClient, type OpencodeClient } from "@opencode-ai/sdk/client"
import { createContext, onCleanup, useContext, type ParentProps } from "solid-js"
import { produce } from "solid-js/store"
import { createActions, type EngineActions } from "./actions"
import { resolveEngine, sleep, type EngineTarget } from "./connection"
import { reduce } from "./events"
import { applySessionSnapshot, applyStatusSnapshot, createRecoveryCoordinator } from "./recovery"
import { streamEvents } from "./sse"
import { seedBench } from "./bench"
import { createEngineState, normalizeDir, type EngineState, type ProviderInfo } from "./store"

export type Engine = { state: EngineState; actions: EngineActions; setDirectory: (path: string | null) => void }

const EngineContext = createContext<Engine>()

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

  const requireClient = () => {
    if (!client) throw new Error("engine offline")
    return client
  }
  const recovery = createRecoveryCoordinator((event, eventDirectory) => reduce(set, event, eventDirectory))
  const actions = createActions(requireClient, state, set, () => base, recovery)

  async function hydrate() {
    const bootDirectory = directory ?? ""
    const api = requireClient()
    const token = recovery.begin()
    try {
      const stale = Object.keys(state.loaded).filter((id) => state.loaded[id])
      const [sessionsResult, statusesResult, providersResult, agentsResult, commandsResult] = await Promise.allSettled([
        api.session.list(),
        api.session.status(),
        api.provider.list(),
        api.app.agents(),
        api.command.list(),
      ])
      const sessions = settledData(sessionsResult)
      const statuses = settledData(statusesResult)
      const providers = settledData(providersResult)
      const agents = settledData(agentsResult)
      const commands = settledData(commandsResult)
      const committed = recovery.commit(token, (events) => {
        if (sessions)
          applySessionSnapshot(
            set,
            sessions,
            (candidate) => normalizeDir(candidate) === normalizeDir(bootDirectory),
            events,
          )
        if (sessions && statuses) applyStatusSnapshot(set, sessions, statuses, events)
        if (providers) {
          set("providers", (providers.all ?? []) as unknown as ProviderInfo[])
          set("connected", providers.connected ?? [])
          set("defaultModels", providers.default ?? {})
        }
        if (agents) set("agents", agents)
        if (commands) set("commands", commands)
      })
      if (committed) await Promise.allSettled(stale.filter((id) => state.sessions[id]).map(actions.reloadSession))
      if (!state.version && base) {
        const health = await fetch(`${base.url}/global/health`, { headers: base.headers })
          .then((response) => (response.ok ? (response.json() as Promise<{ version?: string }>) : null))
          .catch(() => null)
        if (health?.version) set("version", health.version)
      }
    } finally {
      if (client === api && directory === bootDirectory && recovery.current(token))
        set("bootstrappedDirectory", bootDirectory)
    }
  }

  async function pump(target: EngineTarget, signal: AbortSignal) {
    while (!signal.aborted) {
      try {
        await streamEvents(target, signal, (event, eventDirectory) => {
          if (event.type === "server.connected") {
            recovery.advance()
            set("connection", "online")
            void hydrate().catch(() => undefined)
            return
          }
          recovery.record(event, eventDirectory)
        })
      } catch {}
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
    recovery.advance()
    pumpAbort?.abort()
    pumpAbort = undefined
    client = undefined
  }

  function startPump(path: string) {
    if (!base || disposed || pumpAbort) return
    client = createOpencodeClient({ baseUrl: base.url, headers: base.headers, directory: path })
    set(
      produce((s) => {
        s.directory = path
        s.connection = "connecting"
      }),
    )
    if (import.meta.env.DEV) seedBench(set, path)
    pumpAbort = new AbortController()
    void pump(base, pumpAbort.signal)
  }

  function setDirectory(path: string | null) {
    if (path === directory && (client || !path)) return
    const prev = directory
    directory = path
    if (!base || disposed) return
    if (!path) {
      stopPump()
      set(
        produce((s) => {
          s.directory = ""
          s.connection = "idle"
        }),
      )
      return
    }
    if (!pumpAbort || !prev) {
      stopPump()
      startPump(path)
      return
    }
    recovery.advance()
    client = createOpencodeClient({ baseUrl: base.url, headers: base.headers, directory: path })
    set("directory", path)
    if (state.connection === "online") void hydrate().catch(() => undefined)
  }

  void resolveEngine()
    .then(async (target) => {
      base = target
      const health = await fetch(`${target.url}/global/health`, { headers: target.headers })
        .then((response) => (response.ok ? (response.json() as Promise<{ version?: string }>) : null))
        .catch(() => null)
      if (health?.version) set("version", health.version)
      if (directory) startPump(directory)
    })
    .catch((error: unknown) => {
      set("startupError", error instanceof Error ? error.message : String(error))
      if (directory) set("connection", "offline")
    })
  onCleanup(() => {
    disposed = true
    pumpAbort?.abort()
  })

  return <EngineContext.Provider value={{ state, actions, setDirectory }}>{props.children}</EngineContext.Provider>
}

function settledData<T>(result: PromiseSettledResult<{ data?: T; error?: unknown }>): T | undefined {
  if (result.status === "rejected" || result.value.error !== undefined) return
  return result.value.data
}
