import { createOpencodeClient, type OpencodeClient } from "@opencode-ai/sdk/client"
import { createContext, onCleanup, useContext, type ParentProps } from "solid-js"
import { produce } from "solid-js/store"
import { createActions, type EngineActions } from "./actions"
import { resolveEngine, sleep, type EngineTarget } from "./connection"
import { reduce } from "./events"
import { streamEvents } from "./sse"
import { createEngineState, type EngineState, type ProviderInfo } from "./store"

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
  const actions = createActions(requireClient, state, set, () => base)

  async function hydrate() {
    const api = requireClient()
    const stale = Object.keys(state.loaded)
    const [sessions, providers, agents, commands] = await Promise.all([
      api.session.list(),
      api.provider.list(),
      api.app.agents(),
      api.command.list(),
    ])
    for (const session of sessions.data ?? []) set("sessions", session.id, session)
    set("providers", (providers.data?.all ?? []) as unknown as ProviderInfo[])
    set("connected", providers.data?.connected ?? [])
    set("defaultModels", providers.data?.default ?? {})
    set("agents", agents.data ?? [])
    set("commands", commands.data ?? [])
    await Promise.all(stale.map(reload))
  }

  async function reload(id: string) {
    const result = await requireClient().session.messages({ path: { id } })
    set("transcripts", id, result.data ?? [])
  }

  async function pump(target: EngineTarget, dir: string, signal: AbortSignal) {
    while (!signal.aborted) {
      try {
        await streamEvents(target, dir, signal, (event) => {
          if (event.type === "server.connected") {
            set("connection", "online")
            void hydrate()
            return
          }
          reduce(set, event)
        })
      } catch {}
      if (signal.aborted) return
      set("connection", "offline")
      await sleep(1500)
      if (signal.aborted) return
      set("connection", "connecting")
    }
  }

  function reset(dir: string | null) {
    set(
      produce((s) => {
        s.sessions = {}
        s.transcripts = {}
        s.loaded = {}
        s.permissions = {}
        s.todos = {}
        s.status = {}
        s.errors = {}
        s.activity = {}
        s.directory = dir ?? ""
        s.connection = dir ? "connecting" : "idle"
      }),
    )
  }

  function apply() {
    if (!base || disposed) return
    pumpAbort?.abort()
    pumpAbort = undefined
    client = undefined
    reset(directory)
    if (!directory) return
    client = createOpencodeClient({ baseUrl: base.url, headers: base.headers, directory })
    pumpAbort = new AbortController()
    void pump(base, directory, pumpAbort.signal)
  }

  function setDirectory(path: string | null) {
    if (path === directory && client) return
    directory = path
    apply()
  }

  void resolveEngine().then((target) => {
    base = target
    apply()
  })
  onCleanup(() => {
    disposed = true
    pumpAbort?.abort()
  })

  return <EngineContext.Provider value={{ state, actions, setDirectory }}>{props.children}</EngineContext.Provider>
}
