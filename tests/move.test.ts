import { expect, test } from "bun:test"
import type { Event, Session } from "@opencode-ai/sdk/client"
import { createActions, sessionTree } from "../src/engine/actions"
import { reduce } from "../src/engine/events"
import { createEngineState } from "../src/engine/store"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

function session(id: string, directory: string, parentID?: string): Session {
  return {
    id,
    slug: id,
    projectID: "old-project",
    directory,
    parentID,
    title: id,
    version: "test",
    time: { created: 1, updated: 1 },
  } as Session
}

test("sessionTree includes every descendant but not siblings", () => {
  const sessions = [
    session("grandchild", "C:/one", "child"),
    session("child", "C:/one", "root"),
    session("sibling", "C:/one"),
    session("root", "C:/one"),
  ]
  expect(sessionTree(sessions, "root").map((entry) => entry.id)).toEqual(["root", "child", "grandchild"])
})

test("metadata-only session moves retain history and move the descendant tree", async () => {
  const source = "C:/one"
  const destination = "C:/two"
  const sessions = [session("root", source), session("child", source, "root"), session("sibling", source)]
  const [state, set] = createEngineState()
  for (const entry of sessions) set("sessions", entry.id, entry)
  const requests: { id: string; moveChanges: boolean }[] = []
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init)
    const url = new URL(request.url)
    if (request.method === "GET" && url.pathname === "/experimental/session") {
      const directory = url.searchParams.get("directory")
      const data = directory === source ? sessions : sessions.map((entry) => ({ ...entry, directory: destination, projectID: "new-project" }))
      return Response.json(data)
    }
    const body = (await request.json()) as {
      sessionID: string
      destination: { directory: string }
      moveChanges: boolean
    }
    requests.push({ id: body.sessionID, moveChanges: body.moveChanges })
    return new Response(null, { status: 204 })
  }) as typeof fetch

  try {
    const actions = createActions(
      () => ({}) as never,
      state,
      set,
      () => ({ url: "http://engine.test" }),
    )
    expect(await actions.moveSession("root", destination)).toEqual({ ok: true, moved: ["root", "child"] })
    expect(requests).toEqual([
      { id: "root", moveChanges: false },
      { id: "child", moveChanges: false },
    ])
    expect(state.sessions.root.directory).toBe(destination)
    expect(state.sessions.root.projectID).toBe("new-project")
    expect(state.sessions.child.directory).toBe(destination)
    expect(state.sessions.sibling.directory).toBe(source)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("a session that stays busy after the abort wait blocks the move", async () => {
  const source = "C:/one"
  const sessions = [session("root", source)]
  const [state, set] = createEngineState()
  for (const entry of sessions) set("sessions", entry.id, entry)
  set("status", "root", { type: "busy" } as never)
  let aborted = 0
  const moveRequests: string[] = []
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init)
    const url = new URL(request.url)
    if (request.method === "GET" && url.pathname === "/experimental/session") return Response.json(sessions)
    moveRequests.push(url.pathname)
    return new Response(null, { status: 204 })
  }) as typeof fetch

  try {
    const actions = createActions(
      () =>
        ({
          session: {
            abort: async () => {
              aborted += 1
              return {}
            },
          },
        }) as never,
      state,
      set,
      () => ({ url: "http://engine.test" }),
      { waitMs: 200, pollMs: 25 },
    )
    const result = await actions.moveSession("root", "C:/two")
    expect(result.ok).toBe(false)
    expect(result.error).toBe("The session is still busy; stop it before moving.")
    expect(aborted).toBe(1)
    expect(moveRequests).toEqual([])
    expect(state.sessions.root.directory).toBe(source)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("a busy session confirmed idle after abort moves successfully", async () => {
  const source = "C:/one"
  const destination = "C:/two"
  const sessions = [session("root", source)]
  const [state, set] = createEngineState()
  for (const entry of sessions) set("sessions", entry.id, entry)
  set("status", "root", { type: "busy" } as never)
  const moveRequests: string[] = []
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init)
    const url = new URL(request.url)
    if (request.method === "GET" && url.pathname === "/experimental/session") {
      const directory = url.searchParams.get("directory")
      const data = directory === source ? sessions : sessions.map((entry) => ({ ...entry, directory: destination }))
      return Response.json(data)
    }
    const body = (await request.json()) as { sessionID: string }
    moveRequests.push(body.sessionID)
    return new Response(null, { status: 204 })
  }) as typeof fetch

  try {
    const actions = createActions(
      () =>
        ({
          session: {
            abort: async () => {
              set("status", "root", { type: "idle" } as never)
              return {}
            },
          },
        }) as never,
      state,
      set,
      () => ({ url: "http://engine.test" }),
      { waitMs: 200, pollMs: 25 },
    )
    expect(await actions.moveSession("root", destination)).toEqual({ ok: true, moved: ["root"] })
    expect(moveRequests).toEqual(["root"])
    expect(state.sessions.root.directory).toBe(destination)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("a failed rebind rolls back sessions that already moved", async () => {
  const source = "C:/one"
  const destination = "C:/two"
  const sessions = [session("root", source), session("child", source, "root")]
  const [state, set] = createEngineState()
  for (const entry of sessions) set("sessions", entry.id, entry)
  const moveRequests: { id: string; destination: string }[] = []
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init)
    const url = new URL(request.url)
    if (request.method === "GET" && url.pathname === "/experimental/session") {
      const directory = url.searchParams.get("directory")
      return Response.json(directory === source ? sessions : [])
    }
    const body = (await request.json()) as { sessionID: string; destination: { directory: string } }
    moveRequests.push({ id: body.sessionID, destination: body.destination.directory })
    if (body.sessionID === "child" && body.destination.directory === destination)
      return Response.json({ data: { message: "child is stuck" } }, { status: 400 })
    return new Response(null, { status: 204 })
  }) as typeof fetch

  try {
    const actions = createActions(
      () => ({}) as never,
      state,
      set,
      () => ({ url: "http://engine.test" }),
    )
    const result = await actions.moveSession("root", destination)
    expect(result).toEqual({ ok: false, moved: [], error: "child is stuck" })
    expect(moveRequests).toEqual([
      { id: "root", destination },
      { id: "child", destination },
      { id: "root", destination: source },
    ])
    expect(state.sessions.root.directory).toBe(source)
    expect(state.sessions.child.directory).toBe(source)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("a failed rollback reports the sessions left rebound", async () => {
  const source = "C:/one"
  const destination = "C:/two"
  const sessions = [session("root", source), session("child", source, "root")]
  const [state, set] = createEngineState()
  for (const entry of sessions) set("sessions", entry.id, entry)
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init)
    const url = new URL(request.url)
    if (request.method === "GET" && url.pathname === "/experimental/session") return Response.json(sessions)
    const body = (await request.json()) as { sessionID: string; destination: { directory: string } }
    if (body.sessionID === "child")
      return Response.json({ data: { message: "child is stuck" } }, { status: 400 })
    if (body.destination.directory === source)
      return Response.json({ data: { message: "source is unavailable" } }, { status: 400 })
    return new Response(null, { status: 204 })
  }) as typeof fetch

  try {
    const actions = createActions(
      () => ({}) as never,
      state,
      set,
      () => ({ url: "http://engine.test" }),
    )
    expect(await actions.moveSession("root", destination)).toEqual({
      ok: false,
      moved: ["root"],
      error: "child is stuck; rollback failed: source is unavailable",
    })
    expect(state.sessions.root.directory).toBe(destination)
    expect(state.sessions.child.directory).toBe(source)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("session move events update routing metadata and preserve transcript state", () => {
  const [state, set] = createEngineState()
  set("sessions", "root", session("root", "C:/one"))
  set("transcripts", "root", [{ info: { id: "m1" }, parts: [] }] as never)
  reduce(
    set,
    {
      type: "session.next.moved",
      properties: {
        sessionID: "root",
        projectID: "new-project",
        location: { directory: "C:/two" },
        subdirectory: "",
        timestamp: 5,
      },
    } as unknown as Event,
  )
  expect(state.sessions.root.directory).toBe("C:/two")
  expect(state.sessions.root.projectID).toBe("new-project")
  expect(state.sessions.root.time.updated).toBe(5)
  expect(state.transcripts.root).toHaveLength(1)
})
