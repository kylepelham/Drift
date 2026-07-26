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
    session("root", "C:/one"),
    session("child", "C:/one", "root"),
    session("grandchild", "C:/one", "child"),
    session("sibling", "C:/one"),
  ]
  expect(sessionTree(sessions, "root").map((entry) => entry.id)).toEqual(["root", "child", "grandchild"])
})

test("metadata-only session moves retain history and move the descendant tree", async () => {
  const source = "C:/one"
  const destination = "C:/two"
  const sessions = [session("root", source), session("child", source, "root"), session("sibling", source)]
  const [state, set] = createEngineState()
  for (const entry of sessions) set("sessions", entry.id, entry)
  const requests: { id: string; moveChanges: boolean; source: string }[] = []
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init)
    const url = new URL(request.url)
    if (request.method === "GET" && url.pathname === "/experimental/session") {
      const directory = url.searchParams.get("directory")
      const data = directory === source ? sessions : sessions.map((entry) => ({ ...entry, directory: destination, projectID: "new-project" }))
      return Response.json(data)
    }
    if (request.method === "GET" && url.pathname === "/session/status") return Response.json({})
    const body = (await request.json()) as {
      sessionID: string
      destination: { directory: string }
      moveChanges: boolean
    }
    requests.push({
      id: body.sessionID,
      moveChanges: body.moveChanges,
      source: decodeURIComponent(request.headers.get("x-opencode-directory") ?? ""),
    })
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
      { id: "root", moveChanges: false, source },
      { id: "child", moveChanges: false, source },
    ])
    expect(state.sessions.root.directory).toBe(destination)
    expect(state.sessions.root.projectID).toBe("new-project")
    expect(state.sessions.child.directory).toBe(destination)
    expect(state.sessions.sibling.directory).toBe(source)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("session moves reject a tree that is still active before rebinding", async () => {
  const source = "C:/one"
  const destination = "C:/two"
  const sessions = [session("root", source), session("child", source, "root")]
  const [state, set] = createEngineState()
  for (const entry of sessions) set("sessions", entry.id, entry)
  const originalFetch = globalThis.fetch
  let moves = 0
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init)
    const url = new URL(request.url)
    if (url.pathname === "/experimental/session") return Response.json(sessions)
    if (url.pathname === "/session/status") return Response.json({ child: { type: "busy" } })
    moves += 1
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
      moved: [],
      error: "Session child is still active",
    })
    expect(moves).toBe(0)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("a partially failed tree move rolls completed sessions back to their source", async () => {
  const source = "C:/one"
  const destination = "C:/two"
  const sessions = [session("root", source), session("child", source, "root")]
  const [state, set] = createEngineState()
  for (const entry of sessions) set("sessions", entry.id, entry)
  const requests: { id: string; destination: string }[] = []
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init)
    const url = new URL(request.url)
    if (url.pathname === "/experimental/session") return Response.json(sessions)
    if (url.pathname === "/session/status") return Response.json({})
    const body = (await request.json()) as { sessionID: string; destination: { directory: string } }
    requests.push({ id: body.sessionID, destination: body.destination.directory })
    if (body.sessionID === "child")
      return Response.json({ data: { message: "still active" } }, { status: 400 })
    return new Response(null, { status: 204 })
  }) as typeof fetch

  try {
    const actions = createActions(
      () => ({}) as never,
      state,
      set,
      () => ({ url: "http://engine.test" }),
    )
    expect(await actions.moveSession("root", destination)).toEqual({ ok: false, moved: [], error: "still active" })
    expect(requests).toEqual([
      { id: "root", destination },
      { id: "child", destination },
      { id: "root", destination: source },
    ])
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
