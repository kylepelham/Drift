import { afterAll, expect, mock, test } from "bun:test"
import type { Session } from "@opencode-ai/sdk/client"
import { createActions } from "../src/engine/actions"
import { createEngineState } from "../src/engine/store"
import { driftStore as realStore, type ArchivedSession, type DriftStore, type Workspace } from "../src/state/store"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

const day = 24 * 60 * 60 * 1000
const expired = Date.now() - 8 * day
const fresh = Date.now() - 1 * day

// In-memory DriftStore standing in for the SQLite layer. It outlives every purge pass in a test,
// which is exactly the durability the tombstones rely on across an app restart.
let storedWorkspaces: Workspace[] = []
let storedArchived: ArchivedSession[] = []

function seed(input: { workspaces?: Workspace[]; archived?: ArchivedSession[] }) {
  storedWorkspaces = input.workspaces ?? []
  storedArchived = input.archived ?? []
}

const unsupported = async (): Promise<never> => {
  throw new Error("not supported in purge tests")
}

const memoryStore: DriftStore = {
  workspaces: async () => storedWorkspaces.filter((w) => !w.removedAt),
  removedWorkspaces: async () => storedWorkspaces.filter((w) => Boolean(w.removedAt)),
  addWorkspace: unsupported,
  saveWorkspace: unsupported,
  touchWorkspace: async () => undefined,
  removeWorkspace: async (id) => {
    storedWorkspaces = storedWorkspaces.map((w) => (w.id === id ? { ...w, removedAt: Date.now() } : w))
  },
  expiredRemovedWorkspaces: async (before) =>
    storedWorkspaces.filter((w) => w.removedAt && w.removedAt < before),
  forgetWorkspace: async (id) => {
    storedWorkspaces = storedWorkspaces.filter((w) => w.id !== id)
    storedArchived = storedArchived.filter((a) => a.workspaceId !== id)
  },
  archived: async () => [...storedArchived],
  archiveSession: async (sessionId, workspaceId) => {
    storedArchived = [
      ...storedArchived.filter((a) => a.sessionId !== sessionId),
      { sessionId, workspaceId, archivedAt: Date.now() },
    ]
  },
  unarchiveSession: async (sessionId) => {
    storedArchived = storedArchived.filter((a) => a.sessionId !== sessionId)
  },
  expiredArchived: async (before) => storedArchived.filter((a) => a.archivedAt < before).map((a) => a.sessionId),
  mcpSnapshot: unsupported,
  saveMcp: unsupported,
  removeMcp: unsupported,
  approveMcp: unsupported,
  rejectMcp: unsupported,
  revokeMcp: unsupported,
}

mock.module("../src/state/store", () => ({ driftStore: memoryStore }))
afterAll(() => {
  mock.module("../src/state/store", () => ({ driftStore: realStore }))
})

// A real engine actions instance over a fake SDK client, so the purge exercises the same result
// validation the app ships: `failing` simulates a stopped sidecar or an engine-side error.
function fakeEngine() {
  const sessions = new Map<string, Session>()
  const deleted: string[] = []
  const [state, set] = createEngineState()
  const harness = {
    sessions,
    deleted,
    state,
    set,
    failing: null as null | "offline" | "error",
    actions: undefined as unknown as ReturnType<typeof createActions>,
  }
  const client = {
    session: {
      async list({ query }: { query?: { directory?: string } } = {}) {
        if (harness.failing === "offline") throw new Error("engine offline")
        if (harness.failing === "error")
          return { data: undefined, error: { message: "engine error" }, response: { status: 500 } }
        return {
          data: [...sessions.values()].filter((s) => !query?.directory || s.directory === query.directory),
          error: undefined,
          response: { status: 200 },
        }
      },
      async delete({ path }: { path: { id: string } }) {
        if (harness.failing === "offline") throw new Error("engine offline")
        if (harness.failing === "error")
          return { data: undefined, error: { message: "engine error" }, response: { status: 500 } }
        if (!sessions.has(path.id))
          return { data: undefined, error: { message: "not found" }, response: { status: 404 } }
        sessions.delete(path.id)
        deleted.push(path.id)
        return { data: true, error: undefined, response: { status: 200 } }
      },
    },
  }
  harness.actions = createActions(() => client as never, state, set, () => ({ url: "http://engine.test" }))
  return harness
}

function session(id: string, directory: string): Session {
  return { id, directory } as Session
}

test("engine deletion failure retains tombstones and a later retry completes the purge", async () => {
  const engine = fakeEngine()
  engine.sessions.set("s-old", session("s-old", "S:/kept"))
  engine.sessions.set("s-ws", session("s-ws", "S:/gone"))
  seed({
    workspaces: [{ id: "w-gone", path: "S:/gone", name: "Gone", icon: "", lastUsed: 1, removedAt: expired }],
    archived: [{ sessionId: "s-old", workspaceId: "w-live", archivedAt: expired }],
  })
  const { purgeAll } = await import("../src/state/workspaces")

  engine.failing = "offline"
  expect(await purgeAll(engine.actions)).toBeFalse()
  // The tombstones survive the failed pass, so a restarted app still knows what to delete.
  expect((await memoryStore.archived()).map((a) => a.sessionId)).toEqual(["s-old"])
  expect((await memoryStore.removedWorkspaces()).map((w) => w.id)).toEqual(["w-gone"])
  expect(engine.sessions.size).toBe(2)

  // Reconnect: the retained tombstones drive the retry to completion.
  engine.failing = null
  expect(await purgeAll(engine.actions)).toBeTrue()
  expect([...engine.deleted].sort()).toEqual(["s-old", "s-ws"])
  expect(await memoryStore.archived()).toEqual([])
  expect(await memoryStore.removedWorkspaces()).toEqual([])
})

test("successful purge removes engine sessions and metadata together, sparing fresh entries", async () => {
  const engine = fakeEngine()
  engine.sessions.set("s-old", session("s-old", "S:/kept"))
  engine.sessions.set("s-new", session("s-new", "S:/kept"))
  engine.sessions.set("s-recent", session("s-recent", "S:/recent"))
  seed({
    workspaces: [{ id: "w-recent", path: "S:/recent", name: "Recent", icon: "", lastUsed: 1, removedAt: fresh }],
    archived: [
      { sessionId: "s-old", workspaceId: "w-live", archivedAt: expired },
      { sessionId: "s-new", workspaceId: "w-live", archivedAt: fresh },
    ],
  })
  const { purgeAll } = await import("../src/state/workspaces")

  expect(await purgeAll(engine.actions)).toBeTrue()
  expect(engine.deleted).toEqual(["s-old"])
  expect((await memoryStore.archived()).map((a) => a.sessionId)).toEqual(["s-new"])
  expect((await memoryStore.removedWorkspaces()).map((w) => w.id)).toEqual(["w-recent"])
})

test("a tombstone for a session the engine already lost is cleared, not retried forever", async () => {
  const engine = fakeEngine()
  seed({ archived: [{ sessionId: "s-ghost", workspaceId: "w-live", archivedAt: expired }] })
  const { purgeAll } = await import("../src/state/workspaces")

  expect(await purgeAll(engine.actions)).toBeTrue()
  expect(engine.deleted).toEqual([])
  expect(await memoryStore.archived()).toEqual([])
})

test("purgeSession validates the SDK result instead of assuming success", async () => {
  const engine = fakeEngine()
  engine.sessions.set("s1", session("s1", "S:/kept"))
  engine.set("sessions", "s1", session("s1", "S:/kept"))

  engine.failing = "error"
  expect(await engine.actions.purgeSession("s1")).toBeFalse()
  expect(engine.state.sessions["s1"]).toBeDefined()
  expect(engine.sessions.has("s1")).toBeTrue()

  engine.failing = null
  expect(await engine.actions.purgeSession("s1")).toBeTrue()
  expect(engine.state.sessions["s1"]).toBeUndefined()
  expect(engine.sessions.has("s1")).toBeFalse()
})
