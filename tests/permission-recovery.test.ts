import { afterEach, expect, test } from "bun:test"
import type { Permission } from "@opencode-ai/sdk/client"
import { createActions } from "../src/engine/actions"
import { reduce } from "../src/engine/events"
import { createEngineState, type QuestionRequest } from "../src/engine/store"
import type { DriftPermission } from "../src/state/permission-attention"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

const originalFetch = globalThis.fetch
afterEach(() => {
  globalThis.fetch = originalFetch
})

const home = "C:/work"
const other = "C:/other"

function permission(id: string, directory?: string): Permission {
  return {
    id,
    type: "bash",
    sessionID: "session",
    messageID: "message",
    title: id,
    metadata: directory ? { directory } : {},
    time: { created: 1 },
  } as Permission
}

function question(id: string, directory?: string): QuestionRequest {
  return { id, sessionID: "session", questions: [], ...(directory ? { directory } : {}) }
}

function harness() {
  const [state, set] = createEngineState()
  set("directory", home)
  const actions = createActions(
    () => ({}) as never,
    state,
    set,
    () => ({ url: "http://engine.test" }),
  )
  return { state, set, actions }
}

test("permission and question polling failures reconcile independently", async () => {
  const { state, set, actions } = harness()
  set("permissions", "session", [permission("permission", home)])
  set("questions", "session", [question("question", home)])
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const path = new URL(String(input)).pathname
    return path === "/permission" ? new Response("failed", { status: 500 }) : Response.json([])
  }) as typeof fetch

  await actions.refreshPermissions([home])
  expect(state.permissions.session?.map((item) => item.id)).toEqual(["permission"])
  expect(state.questions.session ?? []).toEqual([])

  set("questions", "session", [question("question", home)])
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const path = new URL(String(input)).pathname
    return path === "/question" ? new Response("failed", { status: 500 }) : Response.json([])
  }) as typeof fetch

  await actions.refreshPermissions([home])
  expect(state.permissions.session ?? []).toEqual([])
  expect(state.questions.session?.map((item) => item.id)).toEqual(["question"])
})

test("network and malformed polling responses preserve pending asks", async () => {
  const { state, set, actions } = harness()
  set("permissions", "session", [permission("permission", home)])
  set("questions", "session", [question("question", home)])
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    if (new URL(String(input)).pathname === "/permission") throw new Error("offline")
    return new Response("not-json", { status: 200 })
  }) as typeof fetch

  await actions.refreshPermissions([home])
  expect(state.permissions.session?.map((item) => item.id)).toEqual(["permission"])
  expect(state.questions.session?.map((item) => item.id)).toEqual(["question"])
})

test("successful scoped snapshots remove confirmed asks but preserve unknown and other directories", async () => {
  const { state, set, actions } = harness()
  set("permissions", "session", [permission("home", home), permission("unknown"), permission("other", other)])
  set("questions", "session", [question("home", home), question("unknown"), question("other", other)])
  globalThis.fetch = (async () => Response.json([])) as typeof fetch

  await actions.refreshPermissions([home])
  expect(state.permissions.session?.map((item) => item.id)).toEqual(["unknown", "other"])
  expect(state.questions.session?.map((item) => item.id)).toEqual(["unknown", "other"])
})

test("legacy polling does not remove pending v2 permissions", async () => {
  const { state, set, actions } = harness()
  reduce(
    set,
    {
      type: "permission.v2.asked",
      properties: {
        id: "v2",
        sessionID: "session",
        action: "bash",
        resources: ["git status"],
        save: ["git *"],
        metadata: {},
      },
    } as never,
    home,
  )
  globalThis.fetch = (async () => Response.json([])) as typeof fetch

  await actions.refreshPermissions([home])
  const request = state.permissions.session?.[0] as DriftPermission
  expect(request.id).toBe("v2")
  expect(request.driftProtocol).toBe("v2")
  expect(request.metadata.always).toEqual(["git *"])
})

test("new SSE asks survive an older empty polling snapshot", async () => {
  const { state, set, actions } = harness()
  const pending: ((response: Response) => void)[] = []
  globalThis.fetch = (() => new Promise<Response>((resolve) => pending.push(resolve))) as typeof fetch

  const refresh = actions.refreshPermissions([home])
  reduce(
    set,
    {
      type: "permission.asked",
      properties: { id: "permission", sessionID: "session", permission: "bash", metadata: {} },
    } as never,
    home,
  )
  reduce(
    set,
    { type: "question.asked", properties: { id: "question", sessionID: "session", questions: [] } } as never,
    home,
  )
  for (const resolve of pending) resolve(Response.json([]))
  await refresh

  expect(state.permissions.session?.map((item) => item.id)).toEqual(["permission"])
  expect(state.questions.session?.map((item) => item.id)).toEqual(["question"])

  globalThis.fetch = (async () => Response.json([])) as typeof fetch
  await actions.refreshPermissions([home])
  expect(state.permissions.session ?? []).toEqual([])
  expect(state.questions.session ?? []).toEqual([])
})

test("a newer SSE reply is not resurrected by an older snapshot", async () => {
  const { state, set, actions } = harness()
  set("permissions", "session", [permission("permission", home)])
  const pending: { path: string; resolve: (response: Response) => void }[] = []
  globalThis.fetch = ((input: RequestInfo | URL) =>
    new Promise<Response>((resolve) => pending.push({ path: new URL(String(input)).pathname, resolve }))) as typeof fetch

  const refresh = actions.refreshPermissions([home])
  reduce(
    set,
    { type: "permission.replied", properties: { sessionID: "session", permissionID: "permission" } } as never,
    home,
  )
  for (const request of pending) {
    request.resolve(
      request.path === "/permission"
        ? Response.json([{ id: "permission", sessionID: "session", permission: "bash", metadata: {} }])
        : Response.json([]),
    )
  }
  await refresh

  expect(state.permissions.session ?? []).toEqual([])
})

test("overlapping polling sweeps coalesce active directories and drain new ones", async () => {
  const { actions } = harness()
  let releaseHome!: () => void
  const homePending = new Promise<void>((resolve) => (releaseHome = resolve))
  const requests: string[] = []
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const url = new URL(String(input))
    const directory = url.searchParams.get("directory") ?? ""
    requests.push(`${directory}:${url.pathname}`)
    if (directory === home) await homePending
    return Response.json([])
  }) as typeof fetch

  const first = actions.refreshPermissions([home])
  const second = actions.refreshPermissions([home, other])
  expect(first).toBe(second)
  releaseHome()
  await Promise.all([first, second])

  expect(requests.filter((request) => request.startsWith(`${home}:`))).toHaveLength(2)
  expect(requests.filter((request) => request.startsWith(`${other}:`))).toHaveLength(2)
})
