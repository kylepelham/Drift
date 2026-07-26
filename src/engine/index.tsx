import { createOpencodeClient, type OpencodeClient } from "@opencode-ai/sdk/client"
import { createContext, onCleanup, useContext, type ParentProps } from "solid-js"
import { produce } from "solid-js/store"
import { createActions, type EngineActions } from "./actions"
import { resolveEngine, sleep, type EngineTarget } from "./connection"
import { reduce } from "./events"
import { streamEvents } from "./sse"
import { seedBench } from "./bench"
import { createEngineState, putSessions, type EngineState, type ProviderInfo } from "./store"

export type Engine = { state: EngineState; actions: EngineActions; setDirectory: (path: string | null) => void }

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

  const requireClient = () => {
    if (!client) throw new Error("engine offline")
    return client
  }
  const actions = createActions(requireClient, state, set, () => base)

  async function hydrate() {
    const bootDirectory = directory ?? ""
    const api = requireClient()
    try {
      const stale = Object.keys(state.loaded)
      const [sessions, [statuses, providers, agents, commands]] = await Promise.all([
        api.session.list().then((result) => {
          putSessions(set, result.data ?? [])
          return result
        }),
        Promise.all([api.session.status(), api.provider.list(), api.app.agents(), api.command.list()]),
      ])
      set(
        produce((draft) => {
          const live = statuses.data ?? {}
          for (const session of sessions.data ?? []) draft.status[session.id] = live[session.id] ?? { type: "idle" }
        }),
      )
      set("providers", (providers.data?.all ?? []) as unknown as ProviderInfo[])
      set("connected", providers.data?.connected ?? [])
      set("defaultModels", providers.data?.default ?? {})
      set("agents", agents.data ?? [])
      set("commands", commands.data ?? [])
      await Promise.all(stale.map(reload))
      if (!state.version && base) {
        const health = await fetchEngineVersion(base)
        if (health?.version) set("version", health.version)
      }
    } finally {
      if (client === api && directory === bootDirectory) set("bootstrappedDirectory", bootDirectory)
    }
  }

  // ponytail: reconnect catch-up reloads the tail page only; deep scrollback refetches on demand
  async function reload(id: string) {
    const result = await requireClient().session.messages({ path: { id }, query: { limit: 100 } })
    if (!result.data) return
    const entries = [...result.data].sort((a, b) => a.info.id.localeCompare(b.info.id))
    set("transcripts", id, entries)
    set("cursors", id, result.response?.headers?.get("x-next-cursor") ?? null)
  }

  async function pump(target: EngineTarget, signal: AbortSignal) {
    while (!signal.aborted) {
      try {
        await streamEvents(target, signal, (event, eventDirectory) => {
          if (event.type === "server.connected") {
            set("connection", "online")
            void hydrate().catch(() => undefined)
            return
          }
          reduce(set, event, eventDirectory)
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
        produce((draft) => {
          draft.directory = ""
          draft.connection = "idle"
        }),
      )
      return
    }
    if (!pumpAbort || !prev) {
      stopPump()
      startPump(path)
      return
    }
    client = createOpencodeClient({ baseUrl: base.url, headers: base.headers, directory: path })
    set("directory", path)
    if (state.connection === "online") void hydrate().catch(() => undefined)
  }

  void resolveEngine()
    .then(async (target) => {
      base = target
      const health = await fetchEngineVersion(target)
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
