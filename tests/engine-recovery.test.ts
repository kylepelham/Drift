import { expect, test } from "bun:test"
import type { Event, Session } from "@opencode-ai/sdk/client"
import { createActions } from "../src/engine/actions"
import { reduce } from "../src/engine/events"
import {
  applySessionSnapshot,
  applyStatusSnapshot,
  applyTranscriptSnapshot,
  createRecoveryCoordinator,
} from "../src/engine/recovery"
import { eventInactivityMs, streamEvents } from "../src/engine/sse"
import { createEngineState } from "../src/engine/store"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

function session(id: string, fields: Partial<Session> = {}): Session {
  return {
    id,
    slug: id,
    projectID: "project",
    directory: "C:/work",
    title: id,
    version: "test",
    time: { created: 1, updated: 1 },
    ...fields,
  } as Session
}

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((done, fail) => {
    resolve = done
    reject = fail
  })
  return { promise, resolve, reject }
}

test("permission replies mutate only after confirmed success", async () => {
  const [state, set] = createEngineState()
  set("directory", "C:/active")
  set("permissions", "s1", [
    {
      id: "cross",
      sessionID: "s1",
      type: "bash",
      pattern: ["git status"],
      messageID: "m1",
      title: "Run command",
      metadata: { directory: "C:/other" },
      time: { created: 1 },
    },
  ] as never)
  const originalFetch = globalThis.fetch
  const responses: (Response | Error)[] = [new Error("offline"), new Response(null, { status: 500 }), new Response(null, { status: 204 })]
  globalThis.fetch = (async () => {
    const response = responses.shift()
    if (response instanceof Error) throw response
    return response!
  }) as typeof fetch

  try {
    const actions = createActions(
      () => ({}) as never,
      state,
      set,
      () => ({ url: "http://engine.test" }),
    )
    expect(await actions.replyPermission("s1", "cross", "once")).toBeFalse()
    expect(await actions.replyPermission("s1", "cross", "once")).toBeFalse()
    expect(state.permissions.s1.map((item) => item.id)).toEqual(["cross"])
    expect(state.notices.filter((item) => item.title === "Permission reply failed")).toHaveLength(1)

    expect(await actions.replyPermission("s1", "cross", "once")).toBeTrue()
    expect(state.permissions.s1).toEqual([])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("active-workspace permission replies require a true SDK response", async () => {
  const [state, set] = createEngineState()
  set("directory", "C:/work")
  set("permissions", "s1", [
    {
      id: "local",
      sessionID: "s1",
      type: "read",
      messageID: "m1",
      title: "Read file",
      metadata: { directory: "C:/work" },
      time: { created: 1 },
    },
  ] as never)
  const replies = [{}, { data: true }]
  const actions = createActions(
    () => ({ postSessionIdPermissionsPermissionId: async () => replies.shift() }) as never,
    state,
    set,
    () => ({ url: "http://engine.test" }),
  )

  expect(await actions.replyPermission("s1", "local", "once")).toBeFalse()
  expect(state.permissions.s1).toHaveLength(1)
  expect(await actions.replyPermission("s1", "local", "once")).toBeTrue()
  expect(state.permissions.s1).toEqual([])
})

test("transcript loading retries failures, preserves data, and clears loading", async () => {
  const [state, set] = createEngineState()
  const existing = [{ info: { id: "old", sessionID: "s1", role: "user" }, parts: [] }]
  const fresh = [{ info: { id: "new", sessionID: "s1", role: "user" }, parts: [] }]
  set("transcripts", "s1", existing as never)
  const pending = deferred<unknown>()
  const replies = [
    () => Promise.reject(new Error("network down")),
    () => Promise.resolve({ error: { message: "engine unavailable" } }),
    () => pending.promise,
  ]
  const actions = createActions(
    () => ({ session: { messages: () => replies.shift()!() } }) as never,
    state,
    set,
    () => ({ url: "http://engine.test" }),
  )

  expect(await actions.openSession("s1")).toBeFalse()
  expect(state.loaded.s1).toBeUndefined()
  expect(state.loading.s1).toBeUndefined()
  expect(state.transcripts.s1).toEqual(existing)

  expect(await actions.openSession("s1")).toBeFalse()
  expect(state.loaded.s1).toBeUndefined()
  expect(state.loading.s1).toBeUndefined()
  expect(state.transcripts.s1).toEqual(existing)

  const first = actions.openSession("s1")
  const duplicate = actions.openSession("s1")
  expect(duplicate).toBe(first)
  expect(state.loading.s1).toBeTrue()
  pending.resolve({ data: fresh, response: { headers: new Headers({ "x-next-cursor": "next" }) } })
  expect(await first).toBeTrue()
  expect(state.loading.s1).toBeUndefined()
  expect(state.loaded.s1).toBeTrue()
  expect(state.transcripts.s1).toEqual(fresh)
  expect(state.cursors.s1).toBe("next")
})

test("ask polls distinguish endpoint failures from valid empty snapshots", async () => {
  const [state, set] = createEngineState()
  set("permissions", "s1", [
    {
      id: "p1",
      sessionID: "s1",
      type: "bash",
      messageID: "m1",
      title: "Run",
      metadata: { directory: "C:/work" },
      time: { created: 1 },
    },
  ] as never)
  set("questions", "s1", [
    { id: "q1", sessionID: "s1", questions: [], directory: "C:/work" },
  ])
  const originalFetch = globalThis.fetch
  let round = 0
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const path = new URL(String(input)).pathname
    if (round === 0) return path === "/permission" ? new Response(null, { status: 500 }) : Response.json([])
    if (round === 1) return path === "/permission" ? new Response("not json") : new Response(null, { status: 503 })
    return Response.json([])
  }) as typeof fetch

  try {
    const actions = createActions(
      () => ({}) as never,
      state,
      set,
      () => ({ url: "http://engine.test" }),
    )
    await actions.refreshPermissions(["C:/work"])
    expect(state.permissions.s1.map((item) => item.id)).toEqual(["p1"])
    expect(state.questions.s1).toBeUndefined()

    set("questions", "s1", [{ id: "q1", sessionID: "s1", questions: [], directory: "C:/work" }])
    round = 1
    await actions.refreshPermissions(["C:/work"])
    expect(state.permissions.s1.map((item) => item.id)).toEqual(["p1"])
    expect(state.questions.s1.map((item) => item.id)).toEqual(["q1"])

    round = 2
    await actions.refreshPermissions(["C:/work"])
    expect(state.permissions.s1).toBeUndefined()
    expect(state.questions.s1).toBeUndefined()
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("a stale ask poll cannot erase asks received over SSE", async () => {
  const [state, set] = createEngineState()
  const recovery = createRecoveryCoordinator((event, directory) => reduce(set, event, directory))
  const permission = deferred<Response>()
  const question = deferred<Response>()
  const originalFetch = globalThis.fetch
  let requests = 0
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    requests += 1
    return new URL(String(input)).pathname === "/permission" ? permission.promise : question.promise
  }) as typeof fetch

  try {
    const actions = createActions(
      () => ({}) as never,
      state,
      set,
      () => ({ url: "http://engine.test" }),
      recovery,
    )
    const poll = actions.refreshPermissions(["C:/work"])
    while (requests < 2) await Promise.resolve()
    recovery.record(
      {
        type: "permission.asked",
        properties: { id: "p-new", sessionID: "s1", permission: "bash", metadata: {} },
      } as never,
      "C:/work",
    )
    recovery.record(
      { type: "question.asked", properties: { id: "q-new", sessionID: "s1", questions: [] } } as never,
      "C:/work",
    )
    permission.resolve(Response.json([]))
    question.resolve(Response.json([]))
    await poll

    expect(state.permissions.s1.map((item) => item.id)).toEqual(["p-new"])
    expect(state.questions.s1.map((item) => item.id)).toEqual(["q-new"])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("authoritative snapshots remove ghosts without overwriting newer SSE state", () => {
  const [state, set] = createEngineState()
  set("sessions", "live", session("live", { title: "before" }))
  set("sessions", "ghost", session("ghost"))
  set("status", "live", { type: "idle" })
  set("loaded", "live", true)
  set("transcripts", "live", [
    {
      info: { id: "m1", sessionID: "live", role: "assistant" },
      parts: [{ id: "part", sessionID: "live", messageID: "m1", type: "text", text: "before" }],
    },
  ] as never)
  const recovery = createRecoveryCoordinator((event, directory) => reduce(set, event, directory))
  const token = recovery.begin()
  recovery.record(
    { type: "session.updated", properties: { info: session("live", { title: "from SSE", time: { created: 1, updated: 3 } }) } } as never,
    "C:/work",
  )
  recovery.record(
    { type: "session.status", properties: { sessionID: "live", status: { type: "busy" } } } as never,
    "C:/work",
  )
  recovery.record(
    {
      type: "message.part.updated",
      properties: {
        part: { id: "part", sessionID: "live", messageID: "m1", type: "text", text: "from SSE" },
      },
    } as never,
    "C:/work",
  )
  let transcriptApplied = true
  expect(
    recovery.commit(token, (events) => {
      applySessionSnapshot(set, [session("live", { title: "stale HTTP" })], () => true, events)
      applyStatusSnapshot(set, [session("live")], { live: { type: "idle" } }, events)
      transcriptApplied = applyTranscriptSnapshot(
        state,
        set,
        "live",
        [
          {
            info: { id: "m1", sessionID: "live", role: "assistant" },
            parts: [{ id: "part", sessionID: "live", messageID: "m1", type: "text", text: "stale HTTP" }],
          },
        ] as never,
        null,
        events,
      )
    }),
  ).toBeTrue()

  expect(state.sessions.ghost).toBeUndefined()
  expect(state.sessions.live.title).toBe("from SSE")
  expect(state.status.live.type).toBe("busy")
  expect((state.transcripts.live[0].parts[0] as { text: string }).text).toBe("from SSE")
  expect(transcriptApplied).toBeFalse()
})

test("deleted-session events and workspace generations invalidate stale HTTP", () => {
  const [state, set] = createEngineState()
  const old = session("old")
  set("sessions", "old", old)
  const recovery = createRecoveryCoordinator((event, directory) => reduce(set, event, directory))
  const deletion = recovery.begin()
  recovery.record({ type: "session.deleted", properties: { info: old } } as never, "C:/work")
  recovery.commit(deletion, (events) => applySessionSnapshot(set, [old], () => true, events))
  expect(state.sessions.old).toBeUndefined()

  const previousWorkspace = recovery.begin()
  recovery.advance()
  expect(
    recovery.commit(previousWorkspace, () => {
      set("sessions", "stale", session("stale"))
    }),
  ).toBeFalse()
  expect(state.sessions.stale).toBeUndefined()
})

test("the SSE inactivity watchdog resets on heartbeats and cancels a silent reader", async () => {
  expect(eventInactivityMs).toBeGreaterThan(10_000)
  const originalFetch = globalThis.fetch
  let canceled = false
  let fetchAborted = false
  const timers: ReturnType<typeof setTimeout>[] = []
  const started = performance.now()
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      const heartbeat = new TextEncoder().encode('data: {"payload":{"type":"server.heartbeat","properties":{}}}\n\n')
      timers.push(setTimeout(() => controller.enqueue(heartbeat), 5))
      timers.push(setTimeout(() => controller.enqueue(heartbeat), 20))
    },
    cancel() {
      canceled = true
      for (const timer of timers) clearTimeout(timer)
    },
  })
  globalThis.fetch = (async (_input: RequestInfo | URL, init?: RequestInit) => {
    init?.signal?.addEventListener("abort", () => (fetchAborted = true), { once: true })
    return new Response(body)
  }) as typeof fetch

  try {
    const events: Event[] = []
    await expect(
      streamEvents({ url: "http://engine.test" }, new AbortController().signal, (event) => events.push(event), 30),
    ).rejects.toThrow("event stream inactive")
    expect(performance.now() - started).toBeGreaterThanOrEqual(40)
    expect(events).toEqual([])
    expect(fetchAborted).toBeTrue()
    expect(canceled).toBeTrue()
  } finally {
    for (const timer of timers) clearTimeout(timer)
    globalThis.fetch = originalFetch
  }
})
