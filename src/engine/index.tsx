import { createContext, onCleanup, useContext, type ParentProps } from "solid-js"
import { produce } from "solid-js/store"
import { normalizeDir as normalizeWorkspacePath } from "./store"
import { workspaces } from "../state/workspaces"
import { clearPermissionAttentionFor } from "../state/permission-attention"
import { seedProviderCatalog } from "../state/provider-cache"
import { createActions, errorMessage, type EngineActions } from "./actions"
import { reduce } from "./events"
import { adaptEvent, type WorkspaceIndex } from "./native/adapt"
import { createClient, type Client, type Target } from "./native/client"
import { connectEvents, type EventStream } from "./native/events"
import { resolveTarget } from "./native/target"
import { createEngineState, type EngineState } from "./store"

export type Engine = {
  state: EngineState
  actions: EngineActions
  setDirectory: (path: string | null) => void
  restartEngine: () => Promise<boolean>
  refreshRuntimeMetadata: () => Promise<void>
}

const EngineContext = createContext<Engine>()

export function useEngine() {
  const engine = useContext(EngineContext)
  if (!engine) throw new Error("useEngine outside EngineProvider")
  return engine
}

/** Workspace ids and paths come from the shell's list; the engine shares that table. */
function workspaceIndex(): WorkspaceIndex {
  const all = workspaces()
  return {
    path: (id) => all.find((w) => w.id === id)?.path,
    id: (path) => {
      const wanted = normalizeWorkspacePath(path)
      return all.find((w) => normalizeWorkspacePath(w.path) === wanted)?.id
    },
  }
}

/** Replaces local state from HTTP. Any failure rejects, so the socket keeps its cursor and retries. */
export async function hydrateFrom(actions: Pick<EngineActions, "refreshProviders" | "loadSessions" | "refreshPermissions">, directory: string | null) {
  const [providers] = await Promise.all([actions.refreshProviders(), directory ? actions.loadSessions(directory) : Promise.resolve()])
  if (providers === false) throw new Error("provider catalog unavailable")
  await actions.refreshPermissions()
}

export function EngineProvider(props: ParentProps) {
  const [state, set] = createEngineState()
  seedProviderCatalog(state, set)
  let client: Client | undefined
  let events: EventStream | undefined
  let directory: string | null = null
  let disposed = false

  const requireClient = () => {
    if (!client) throw new Error("engine offline")
    return client
  }
  const actions = createActions(requireClient, state, set, workspaceIndex)

  async function hydrate() {
    if (!client || disposed) return
    set(
      produce((draft) => {
        draft.sessionSnapshotEpoch += 1
        // Cached transcripts may have missed events; going offline and back makes every open view refetch.
        draft.connection = "connecting"
        draft.loaded = {}
      }),
    )
    await hydrateFrom(actions, directory)
    if (!disposed) set("connection", "online")
  }

  function connect(next: Target) {
    client = createClient(next)
    set("startupError", "")
    set("engineError", "")
    events?.close()
    events = connectEvents(next, {
      hydrate: () => hydrate(),
      event: (envelope) => {
        const legacy = adaptEvent(envelope, workspaceIndex())
        if (legacy) reduce(set, legacy, directory ?? undefined, state)
      },
      online: (online) => {
        set("nativeOnline", online)
        if (!online && state.connection === "online") set("connection", "offline")
      },
    })
  }

  async function start() {
    set("connection", "connecting")
    try {
      const next = await resolveTarget()
      if (disposed) return false
      const health = await createClient(next).health()
      set("version", health.version)
      set("nativeVersion", health.version)
      connect(next)
      return true
    } catch (cause) {
      const message = errorMessage(cause)
      set(
        produce((draft) => {
          draft.startupError = message
          draft.engineError = message
          draft.connection = "offline"
        }),
      )
      return false
    }
  }

  function setDirectory(path: string | null) {
    directory = path
    set("directory", path ?? "")
    if (!path) return
    if (client && state.connection === "online") void actions.loadSessions(path).catch(() => undefined)
  }

  async function restartEngine() {
    set("engineRestarting", true)
    try {
      return await start()
    } finally {
      set("engineRestarting", false)
    }
  }

  async function refreshRuntimeMetadata() {
    if (!client) return
    await actions.refreshProviders().catch(() => undefined)
  }

  void start()
  onCleanup(() => {
    disposed = true
    events?.close()
    clearPermissionAttentionFor(Object.values(state.permissions).flat())
  })

  return (
    <EngineContext.Provider value={{ state, actions, setDirectory, restartEngine, refreshRuntimeMetadata }}>
      {props.children}
    </EngineContext.Provider>
  )
}
