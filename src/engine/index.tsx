import { createContext, onCleanup, untrack, useContext, type ParentProps } from "solid-js"
import { produce, reconcile } from "solid-js/store"
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
import { createEngineState, putTasks, type EngineState } from "./store"

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
export async function hydrateFrom(actions: Pick<EngineActions, "refreshProviders" | "loadSessions" | "refreshPermissions" | "refreshAgents" | "refreshMcp">, directory: string | null) {
  const [providers] = await Promise.all([
    actions.refreshProviders(),
    directory ? actions.loadSessions(directory) : Promise.resolve(),
    directory ? actions.refreshAgents() : Promise.resolve(),
    actions.refreshMcp(),
  ])
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
  // Workspaces whose conversation list this connection already holds; events keep each current, so
  // switching back needs no reload. A hydrate starts over.
  const listed = new Set<string>()

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
    listed.clear()
    const hydrating = directory
    await hydrateFrom(actions, hydrating)
    if (hydrating) listed.add(hydrating)
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
        if (envelope.type === "catalog.updated") void actions.refreshProviders().catch(() => undefined)
        if (envelope.type === "task.updated") putTasks(set, state, envelope.task.parentSessionId, [envelope.task])
        if (envelope.type === "mcp.updated") set("mcpServers", envelope.server.name, reconcile(envelope.server))
        if (envelope.type === "mcp.removed") set("mcpServers", produce((servers) => void delete servers[envelope.name]))
        const legacy = adaptEvent(envelope, workspaceIndex())
        if (legacy) reduce(set, legacy, directory ?? undefined, state, (id) => void actions.reconcileSession(id))
      },
      online: (online) => {
        set("nativeOnline", online)
        if (!online && state.connection === "online") set("connection", "offline")
      },
      // A resume replays what was missed, so the views stay as they are and only come back online.
      resumed: () => {
        if (state.connection === "offline") set("connection", "online")
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

  // Untracked: callers run this from effects, and the loads it starts read the state they later write.
  function setDirectory(path: string | null) {
    untrack(() => applyDirectory(path))
  }

  function applyDirectory(path: string | null) {
    directory = path
    set("directory", path ?? "")
    if (!path) return
    if (client && state.connection === "online") {
      if (!listed.has(path)) {
        listed.add(path)
        void actions.loadSessions(path).catch(() => listed.delete(path))
      }
      void actions.refreshAgents().catch(() => undefined)
    }
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
    await Promise.all([actions.refreshProviders(), actions.refreshAgents()]).catch(() => undefined)
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
