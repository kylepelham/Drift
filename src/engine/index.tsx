import { createOpencodeClient, type OpencodeClient } from "@opencode-ai/sdk/client"
import { createContext, onCleanup, useContext, type ParentProps } from "solid-js"
import { createActions, type EngineActions } from "./actions"
import { resolveEngine, sleep, type EngineTarget } from "./connection"
import { reduce } from "./events"
import { streamEvents } from "./sse"
import { createEngineState, type EngineState, type ProviderInfo } from "./store"

export type Engine = { state: EngineState; actions: EngineActions }

const EngineContext = createContext<Engine>()

export function useEngine() {
  const engine = useContext(EngineContext)
  if (!engine) throw new Error("useEngine outside EngineProvider")
  return engine
}

export function EngineProvider(props: ParentProps) {
  const [state, set] = createEngineState()
  let client: OpencodeClient | undefined

  const requireClient = () => {
    if (!client) throw new Error("engine offline")
    return client
  }
  const actions = createActions(requireClient, state, set)

  async function hydrate() {
    const api = requireClient()
    const [sessions, providers, agents, path] = await Promise.all([
      api.session.list(),
      api.provider.list(),
      api.app.agents(),
      api.path.get(),
    ])
    set("sessions", Object.fromEntries((sessions.data ?? []).map((s) => [s.id, s])))
    set("providers", (providers.data?.all ?? []) as unknown as ProviderInfo[])
    set("connected", providers.data?.connected ?? [])
    set("defaultModels", providers.data?.default ?? {})
    set("agents", agents.data ?? [])
    set("directory", path.data?.directory ?? "")
    await Promise.all(Object.keys(state.loaded).map(reload))
  }

  async function reload(id: string) {
    const result = await requireClient().session.messages({ path: { id } })
    set("transcripts", id, result.data ?? [])
  }

  const aborter = new AbortController()

  async function pump(target: EngineTarget) {
    while (!aborter.signal.aborted) {
      try {
        await streamEvents(target, aborter.signal, (event) => {
          if (event.type === "server.connected") {
            set("connection", "online")
            void hydrate()
            return
          }
          reduce(set, event)
        })
      } catch {}
      if (aborter.signal.aborted) return
      set("connection", "offline")
      await sleep(1500)
      set("connection", "connecting")
    }
  }

  void resolveEngine().then((target) => {
    client = createOpencodeClient({ baseUrl: target.url, headers: target.headers })
    void pump(target)
  })
  onCleanup(() => aborter.abort())

  return <EngineContext.Provider value={{ state, actions }}>{props.children}</EngineContext.Provider>
}
