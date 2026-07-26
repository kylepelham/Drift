import { createOpencodeClient, type OpencodeClient } from "@opencode-ai/sdk/client"
import { createContext, onCleanup, useContext, type ParentProps } from "solid-js"
import { produce } from "solid-js/store"
import { createActions, sessionListComplete, sessionListQuery, type EngineActions } from "./actions"
import { resolveEngine, sleep, type EngineTarget } from "./connection"
import { reduce } from "./events"
import {
  applySessionSnapshot,
  applyStatusSnapshot,
  createRecoveryCoordinator,
  eventInDirectory,
  isSessionEvent,
  isStatusEvent,
} from "./recovery"
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
    const token = recovery.begin(
      (entry) => (isSessionEvent(entry.event) || isStatusEvent(entry.event)) && eventInDirectory(entry, bootDirectory),
    )
    try {
      const stale = Object.keys(state.loaded).filter((id) => state.loaded[id])
      const [sessionsResult, statusesResult, providersResult, agentsResult, commandsResult, healthResult] =
        await Promise.allSettled([
          api.session.list({ query: sessionListQuery(), signal: token.signal }),
          api.session.status({ signal: token.signal }),
          api.provider.list({ signal: token.signal }),
          api.app.agents({ signal: token.signal }),
          api.command.list({ signal: token.signal }),
          base ? loadHealth(base, token.signal) : Promise.resolve(null),
        ])
      const sessions = settledData(sessionsResult)
      const statuses = settledData(statusesResult)
      const providers = settledData(providersResult)
      const agents = settledData(agentsResult)
      const commands = settledData(commandsResult)
      const health = healthResult.status === "fulfilled" ? healthResult.value : null
      const committed = recovery.commit(token, (events) => {
        if (sessions)
          applySessionSnapshot(
            set,
            sessions,
            sessionListComplete(sessions),
            (candidate) => normalizeDir(candidate) === normalizeDir(bootDirectory),
            events,
            recovery.replay,
          )
        if (sessions && statuses) applyStatusSnapshot(set, sessions, statuses, events)
        if (providers) {
          set("providers", (providers.all ?? []) as unknown as ProviderInfo[])
          set("connected", providers.connected ?? [])
          set("defaultModels", providers.default ?? {})
        }
        if (agents) set("agents", agents)
        if (commands) set("commands", commands)
        if (health?.version) set("version", health.version)
      })
      if (committed) await Promise.allSettled(stale.filter((id) => state.sessions[id]).map(actions.reloadSession))
    } finally {
      if (client === api && directory === bootDirectory && recovery.current(token))
        set("bootstrappedDirectory", bootDirectory)
      recovery.cancel(token)
    }
  }

  async function pump(target: EngineTarget, signal: AbortSignal) {
    while (!signal.aborted) {
      try {
        await streamEvents(
          target,
          signal,
          (event, eventDirectory) => {
            if (event.type === "server.connected") {
              recovery.advance()
              set("connection", "online")
              void hydrate().catch(() => undefined)
              return
            }
            recovery.record(event, eventDirectory)
          },
          undefined,
          recovery.advance,
        )
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
      const token = recovery.begin(() => false)
      try {
        const health = await loadHealth(target, token.signal)
        recovery.commit(token, () => {
          if (health?.version) set("version", health.version)
        })
      } finally {
        recovery.cancel(token)
      }
      if (directory) startPump(directory)
    })
    .catch((error: unknown) => {
      set("startupError", error instanceof Error ? error.message : String(error))
      if (directory) set("connection", "offline")
    })
  onCleanup(() => {
    disposed = true
    recovery.advance()
    pumpAbort?.abort()
  })

  return <EngineContext.Provider value={{ state, actions, setDirectory }}>{props.children}</EngineContext.Provider>
}

function settledData<T>(result: PromiseSettledResult<{ data?: T; error?: unknown }>): T | undefined {
  if (result.status === "rejected" || result.value.error !== undefined) return
  return result.value.data
}

async function loadHealth(target: EngineTarget, signal: AbortSignal) {
  return fetch(`${target.url}/global/health`, { headers: target.headers, signal })
    .then((response) => (response.ok ? (response.json() as Promise<{ version?: string }>) : null))
    .catch(() => null)
}
