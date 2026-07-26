import type { McpStatus } from "@opencode-ai/sdk/client"
import { createStore } from "solid-js/store"
import type { DriftStore, McpConfig, McpSnapshot, ObservedMcpServer } from "./store"

export type McpExactTarget = ObservedMcpServer & {
  directory: string
  generation: number
}

export type McpStoredExpectation = {
  generation: number
  previousName?: string
  updatedAt?: number
}

export type McpCoordinatorState = {
  directory: string
  online: boolean
  snapshot: McpSnapshot
  statuses: Record<string, McpStatus>
  loading: boolean
  ready: boolean
  mutation: string | null
  error: string
}

export type McpCoordinatorDependencies = {
  store: DriftStore
  status: (directory: string) => Promise<Record<string, McpStatus>>
  connect: (name: string, directory: string) => Promise<void>
  disconnect: (name: string, directory: string) => Promise<void>
  authenticate: (name: string, directory: string) => Promise<void>
  listen?: (refresh: () => void) => Promise<() => void>
  eventDebounceMs?: number
}

type RefreshContext = { directory: string; online: boolean; revision: number }

const emptySnapshot = (directory = ""): McpSnapshot => ({ generation: 0, directory, servers: [], observed: [] })

export function exactMcpTarget(snapshot: McpSnapshot, observed: ObservedMcpServer): McpExactTarget {
  return { ...observed, directory: snapshot.directory, generation: snapshot.generation }
}

function hasExactMcpTarget(snapshot: McpSnapshot, target: McpExactTarget) {
  return (
    snapshot.generation === target.generation &&
    snapshot.directory === target.directory &&
    snapshot.observed.some(
      (item) =>
        item.name === target.name &&
        item.type === target.type &&
        item.fingerprint === target.fingerprint &&
        item.decision === target.decision,
    )
  )
}

function hasExpectedMcpServer(snapshot: McpSnapshot, name: string, expected: McpStoredExpectation) {
  if (snapshot.generation !== expected.generation) return false
  if (!expected.previousName) return !snapshot.servers.some((server) => server.name === name)
  const current = snapshot.servers.find((server) => server.name === expected.previousName)
  const destinationAvailable =
    name === expected.previousName || !snapshot.servers.some((server) => server.name === name)
  return !!current && current.updatedAt === expected.updatedAt && destinationAvailable
}

export function mcpSnapshotActionable(
  state: Pick<McpCoordinatorState, "directory" | "snapshot" | "loading" | "ready">,
) {
  return state.ready && !state.loading && state.snapshot.directory === state.directory
}

export function mcpFingerprintId(fingerprint: string) {
  return fingerprint.replace(/^sha256:/, "").slice(0, 12)
}

export function createMcpRefreshDebouncer(
  run: () => void,
  delay = 100,
  timers: {
    set: (callback: () => void, delay: number) => ReturnType<typeof setTimeout>
    clear: (timer: ReturnType<typeof setTimeout>) => void
  } = { set: setTimeout, clear: clearTimeout },
) {
  let timer: ReturnType<typeof setTimeout> | undefined
  return {
    trigger() {
      if (timer !== undefined) timers.clear(timer)
      timer = timers.set(() => {
        timer = undefined
        run()
      }, delay)
    },
    cancel() {
      if (timer !== undefined) timers.clear(timer)
      timer = undefined
    },
  }
}

export function createMcpCoordinator(initial?: McpCoordinatorDependencies) {
  const [state, setState] = createStore<McpCoordinatorState>({
    directory: "",
    online: false,
    snapshot: emptySnapshot(),
    statuses: {},
    loading: false,
    ready: false,
    mutation: null,
    error: "",
  })
  let dependencies = initial
  let tail = Promise.resolve()
  let stopListening: (() => void) | undefined
  let stopDebounce: (() => void) | undefined
  let startSequence = 0
  let revision = 0
  let mutationToken: symbol | undefined

  function serialize<T>(work: () => Promise<T>) {
    const task = tail.then(work, work)
    tail = task.then(
      () => undefined,
      () => undefined,
    )
    return task
  }

  function requireDependencies() {
    if (!dependencies) throw new Error("MCP coordinator is not initialized")
    return dependencies
  }

  function context(): RefreshContext {
    return { directory: state.directory, online: state.online, revision }
  }

  function current(request: RefreshContext) {
    return request.revision === revision && request.directory === state.directory && request.online === state.online
  }

  function clearForContext(directory: string) {
    setState("snapshot", emptySnapshot(directory))
    setState("statuses", {})
    setState("ready", false)
    setState("error", "")
  }

  async function refreshUnlocked(request = context()) {
    const api = requireDependencies()
    if (!current(request)) return emptySnapshot(request.directory)
    setState("loading", true)
    setState("ready", false)
    setState("error", "")
    try {
      let statuses: Record<string, McpStatus> = {}
      if (request.directory && request.online) {
        statuses = await api.status(request.directory)
      }
      const snapshot = await api.store.mcpSnapshot(request.directory)
      if (!current(request)) return snapshot
      setState("snapshot", snapshot)
      setState("statuses", statuses)
      setState("ready", true)
      return snapshot
    } catch (error) {
      const message = conciseMcpError(error)
      if (current(request)) {
        setState("snapshot", emptySnapshot(request.directory))
        setState("statuses", {})
        setState("ready", false)
        setState("error", message)
      }
      throw new Error(message)
    } finally {
      if (current(request)) setState("loading", false)
    }
  }

  async function refreshStatusUnlocked(request = context()) {
    if (!current(request) || !request.directory || !request.online) return {}
    const statuses = await requireDependencies().status(request.directory)
    if (current(request)) setState("statuses", statuses)
    return statuses
  }

  function refresh() {
    const request = context()
    return serialize(() => refreshUnlocked(request))
  }

  function refreshStatus() {
    const request = context()
    return serialize(() => refreshStatusUnlocked(request))
  }

  function setActive(directory: string, online: boolean) {
    if (state.directory === directory && state.online === online) return Promise.resolve(state.snapshot)
    revision++
    setState("directory", directory)
    setState("online", online)
    clearForContext(directory)
    setState("loading", true)
    const request = context()
    return serialize(() => refreshUnlocked(request))
  }

  function assertReady() {
    if (!mcpSnapshotActionable(state))
      throw new Error("MCP definitions are refreshing. Review the latest state before retrying.")
  }

  function assertExact(target: McpExactTarget) {
    assertReady()
    if (!hasExactMcpTarget(state.snapshot, target))
      throw new Error("This MCP request is stale. Review the latest definition.")
  }

  function assertStored(name: string, expected: McpStoredExpectation) {
    assertReady()
    if (!hasExpectedMcpServer(state.snapshot, name, expected))
      throw new Error("This MCP server changed while the editor was open. Reopen it and review the latest definition.")
  }

  function mutation(label: string, work: (api: McpCoordinatorDependencies) => Promise<void>) {
    if (state.mutation) return Promise.reject(new Error("Another MCP operation is already in progress."))
    const token = Symbol(label)
    mutationToken = token
    setState("mutation", label)
    return serialize(async () => {
      const api = requireDependencies()
      setState("error", "")
      try {
        try {
          await work(api)
        } catch (error) {
          const message = conciseMcpError(error)
          await refreshUnlocked().catch(() => undefined)
          setState("error", message)
          throw new Error(message)
        }
        try {
          await refreshUnlocked()
        } catch {
          // The native mutation committed; keep its success distinct from a failed status refresh.
        }
      } finally {
        if (mutationToken === token) {
          mutationToken = undefined
          setState("mutation", null)
        }
      }
    })
  }

  function save(name: string, config: McpConfig, expected: McpStoredExpectation) {
    try {
      assertStored(name, expected)
    } catch (error) {
      return Promise.reject(error)
    }
    return mutation(name, async (api) => {
      assertStored(name, expected)
      await api.store.saveMcp(name, config, expected.generation, expected.previousName)
    })
  }

  function remove(name: string, expected: McpStoredExpectation) {
    try {
      assertStored(name, expected)
    } catch (error) {
      return Promise.reject(error)
    }
    return mutation(name, async (api) => {
      assertStored(name, expected)
      await api.store.removeMcp(name, expected.generation)
    })
  }

  function decide(action: "approve" | "reject" | "revoke", target: McpExactTarget) {
    try {
      assertExact(target)
      if (action !== "revoke" && target.decision !== "pending")
        throw new Error("This MCP request is no longer pending.")
      if (action === "revoke" && target.decision !== "approved" && target.decision !== "rejected")
        throw new Error("This MCP definition has no decision to revoke.")
    } catch (error) {
      return Promise.reject(error)
    }
    return mutation(target.name, async (api) => {
      assertExact(target)
      const args = [target.directory, target.name, target.fingerprint, target.generation] as const
      if (action === "approve") await api.store.approveMcp(...args)
      else if (action === "reject") await api.store.rejectMcp(...args)
      else await api.store.revokeMcp(...args)
    })
  }

  function runtime(target: McpExactTarget, action: "connect" | "disconnect" | "authenticate") {
    try {
      assertExact(target)
      if (!state.online || target.decision !== "approved")
        throw new Error("This MCP runtime target is no longer available.")
    } catch (error) {
      return Promise.reject(error)
    }
    return mutation(target.name, async (api) => {
      assertExact(target)
      if (!state.online || target.decision !== "approved")
        throw new Error("This MCP runtime target is no longer available.")
      if (action === "connect") await api.connect(target.name, target.directory)
      else if (action === "disconnect") await api.disconnect(target.name, target.directory)
      else await api.authenticate(target.name, target.directory)
    })
  }

  function start(next: McpCoordinatorDependencies) {
    dependencies = next
    const sequence = ++startSequence
    stopListening?.()
    stopListening = undefined
    stopDebounce?.()
    revision++
    clearForContext(state.directory)
    setState("loading", true)
    const initial = context()
    void serialize(() => refreshUnlocked(initial)).catch(() => undefined)

    const debounce = createMcpRefreshDebouncer(
      () => void refresh().catch(() => undefined),
      next.eventDebounceMs ?? 100,
    )
    stopDebounce = debounce.cancel
    if (next.listen) {
      void next
        .listen(debounce.trigger)
        .then((stop) => {
          if (sequence !== startSequence) return stop()
          stopListening = stop
        })
        .catch(() => undefined)
    }
    return () => {
      if (sequence !== startSequence) return
      startSequence++
      debounce.cancel()
      stopDebounce = undefined
      stopListening?.()
      stopListening = undefined
    }
  }

  return { state, start, setActive, refresh, refreshStatus, save, remove, decide, runtime, settled: () => tail }
}

function conciseMcpError(error: unknown) {
  const message = error instanceof Error ? error.message : String(error)
  return message.replace(/^Error:\s*/i, "").trim() || "MCP operation failed"
}

export const mcpCoordinator = createMcpCoordinator()
