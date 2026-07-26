import { expect, test } from "bun:test"
import type { Event, Session } from "@opencode-ai/sdk/client"
import { createActions } from "../src/engine/actions"
import { reduce } from "../src/engine/events"
import {
  applySessionSnapshot,
  applyStatusSnapshot,
  applyTranscriptSnapshot,
  createRecoveryCoordinator,
  eventInDirectory,
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

test("permission refreshes restart their full directory batch after a generation change", async () => {
  const [state, set] = createEngineState()
  const recovery = createRecoveryCoordinator((event, directory) => reduce(set, event, directory))
  const originalFetch = globalThis.fetch
  const counts = new Map<string, number>()
  globalThis.fetch = ((input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input))
    const key = `${url.pathname}|${url.searchParams.get("directory")}`
    const count = (counts.get(key) ?? 0) + 1
    counts.set(key, count)
    if (url.searchParams.get("directory") !== "C:/one" || count > 1) return Promise.resolve(Response.json([]))
    return new Promise<Response>((resolve) => {
      const abort = () => resolve(new Response(null, { status: 503 }))
      if (init?.signal?.aborted) abort()
      else init?.signal?.addEventListener("abort", abort, { once: true })
    })
  }) as typeof fetch

  try {
    const actions = createActions(
      () => ({}) as never,
      state,
      set,
      () => ({ url: "http://engine.test" }),
      recovery,
    )
    const refresh = actions.refreshPermissions(["C:/one", "C:/two"])
    while ((counts.get("/permission|C:/one") ?? 0) < 1 || (counts.get("/question|C:/one") ?? 0) < 1)
      await Promise.resolve()
    recovery.advance()
    const reconnectRefresh = actions.refreshPermissions(["C:/one"])
    await Promise.all([refresh, reconnectRefresh])

    expect(counts.get("/permission|C:/one")).toBe(2)
    expect(counts.get("/question|C:/one")).toBe(2)
    expect(counts.get("/permission|C:/two")).toBe(1)
    expect(counts.get("/question|C:/two")).toBe(1)
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
  let transcriptApplied: ReturnType<typeof applyTranscriptSnapshot> | undefined
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
  expect(transcriptApplied).toBe("applied")
})

test("overlapping transcript snapshots preserve live deltas and apply activity side effects once", () => {
  const [state, set] = createEngineState()
  set("loaded", "s1", true)
  set("transcripts", "s1", [
    {
      info: { id: "m3", sessionID: "s1", role: "assistant" },
      parts: [{ id: "p3", sessionID: "s1", messageID: "m3", type: "text", text: "tail" }],
    },
  ] as never)
  const recovery = createRecoveryCoordinator((event, directory) => reduce(set, event, directory))
  const token = recovery.begin()
  recovery.record(
    {
      type: "message.part.delta",
      properties: { sessionID: "s1", messageID: "m3", partID: "p3", field: "text", delta: "!" },
    } as never,
  )
  for (const [id, tool] of [
    ["tool-1", "read"],
    ["tool-2", "bash"],
  ]) {
    recovery.record({
      type: "message.part.updated",
      properties: {
        part: {
          id,
          sessionID: "s1",
          messageID: "m4",
          type: "tool",
          callID: id,
          tool,
          state: { status: "running", input: {}, time: { start: 1 } },
        },
      },
    } as never)
  }

  recovery.commit(token, (events) =>
    applyTranscriptSnapshot(
      state,
      set,
      "s1",
      [
        {
          info: { id: "m2", sessionID: "s1", role: "user" },
          parts: [{ id: "p2", sessionID: "s1", messageID: "m2", type: "text", text: "missed" }],
        },
        {
          info: { id: "m3", sessionID: "s1", role: "assistant" },
          parts: [{ id: "p3", sessionID: "s1", messageID: "m3", type: "text", text: "tail!" }],
        },
        { info: { id: "m4", sessionID: "s1", role: "assistant" }, parts: [] },
      ] as never,
      null,
      events,
    ),
  )

  expect(state.transcripts.s1.map((entry) => entry.info.id)).toEqual(["m2", "m3", "m4"])
  expect((state.transcripts.s1[1].parts[0] as { text: string }).text).toBe("tail!")
  expect(state.activity.s1.tools).toBe(2)
  expect(state.transcripts.s1[2].parts.map((part) => part.id)).toEqual(["tool-1", "tool-2"])
})

test("an initially unloaded transcript retries across non-idempotent stream deltas", async () => {
  const [state, set] = createEngineState()
  const recovery = createRecoveryCoordinator((event, directory) => reduce(set, event, directory))
  const first = deferred<unknown>()
  let requests = 0
  const actions = createActions(
    () => ({
      session: {
        messages: () => {
          requests += 1
          if (requests === 1) return first.promise
          return Promise.resolve({
            data: [
              {
                info: { id: "m1", sessionID: "s1", role: "assistant" },
                parts: [{ id: "p1", sessionID: "s1", messageID: "m1", type: "text", text: "live event" }],
              },
            ],
            response: { headers: new Headers() },
          })
        },
      },
    }) as never,
    state,
    set,
    () => ({ url: "http://engine.test" }),
    recovery,
  )
  const loading = actions.openSession("s1")
  while (requests < 1) await Promise.resolve()
  recovery.record(
    { type: "message.updated", properties: { info: { id: "m1", sessionID: "s1", role: "assistant" } } } as never,
  )
  recovery.record(
    {
      type: "message.part.updated",
      properties: { part: { id: "p1", sessionID: "s1", messageID: "m1", type: "text", text: "live" } },
    } as never,
  )
  recovery.record(
    {
      type: "message.part.delta",
      properties: { sessionID: "s1", messageID: "m1", partID: "p1", field: "text", delta: " event" },
    } as never,
  )
  expect(state.transcripts.s1).toBeUndefined()
  first.resolve({
    data: [
      {
        info: { id: "m1", sessionID: "s1", role: "assistant" },
        parts: [{ id: "p1", sessionID: "s1", messageID: "m1", type: "text", text: "live event" }],
      },
    ],
    response: { headers: new Headers() },
  })

  expect(await loading).toBeTrue()
  expect(requests).toBe(2)
  expect(state.loaded.s1).toBeTrue()
  expect((state.transcripts.s1[0].parts[0] as { text: string }).text).toBe("live event")
})

test("a reconnect does not coalesce transcript loads onto the aborted generation", async () => {
  const [state, set] = createEngineState()
  const recovery = createRecoveryCoordinator((event, directory) => reduce(set, event, directory))
  let calls = 0
  let firstSignal: AbortSignal | undefined
  const actions = createActions(
    () => ({
      session: {
        messages: (options: { signal: AbortSignal }) => {
          calls += 1
          if (calls === 1) {
            firstSignal = options.signal
            return new Promise((_, reject) =>
              options.signal.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")), {
                once: true,
              }),
            )
          }
          return Promise.resolve({
            data: [{ info: { id: "fresh", sessionID: "s1", role: "user" }, parts: [] }],
            response: { headers: new Headers() },
          })
        },
      },
    }) as never,
    state,
    set,
    () => ({ url: "http://engine.test" }),
    recovery,
  )

  const stale = actions.openSession("s1")
  while (calls < 1) await Promise.resolve()
  recovery.advance()
  const fresh = actions.openSession("s1")

  expect(fresh).not.toBe(stale)
  expect(await stale).toBeFalse()
  expect(await fresh).toBeTrue()
  expect(firstSignal?.aborted).toBeTrue()
  expect(calls).toBe(2)
  expect(state.transcripts.s1[0].info.id).toBe("fresh")
})

test("a reconnect starts a new all-sessions request while the old request unwinds", async () => {
  const [state, set] = createEngineState()
  const recovery = createRecoveryCoordinator((event, directory) => reduce(set, event, directory))
  const originalFetch = globalThis.fetch
  let calls = 0
  let firstSignal: AbortSignal | undefined
  globalThis.fetch = (async (_input: RequestInfo | URL, init?: RequestInit) => {
    calls += 1
    if (calls === 1) {
      firstSignal = init?.signal ?? undefined
      return new Promise<Response>((resolve) =>
        init?.signal?.addEventListener("abort", () => resolve(new Response(null, { status: 503 })), { once: true }),
      )
    }
    return Response.json([session("fresh")])
  }) as typeof fetch

  try {
    const actions = createActions(
      () => ({}) as never,
      state,
      set,
      () => ({ url: "http://engine.test" }),
      recovery,
    )
    const stale = actions.loadAllSessions()
    while (calls < 1) await Promise.resolve()
    recovery.advance()
    const fresh = actions.loadAllSessions()
    await Promise.all([stale, fresh])

    expect(firstSignal?.aborted).toBeTrue()
    expect(calls).toBe(2)
    expect(state.sessions.fresh.id).toBe("fresh")
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("unrelated event overflow cannot invalidate a scoped recovery request", () => {
  const recovery = createRecoveryCoordinator(() => undefined, 1)
  const token = recovery.begin(
    (entry) => entry.event.type === "message.updated" && entry.event.properties.info.sessionID === "s1",
  )
  for (let index = 0; index < 100; index += 1)
    recovery.record({ type: "session.status", properties: { sessionID: `other-${index}`, status: { type: "idle" } } } as never)

  let committed = false
  expect(recovery.commit(token, () => (committed = true))).toBeTrue()
  expect(committed).toBeTrue()
})

test("directory session loads ignore unrelated churn and include move source and destination", async () => {
  const [state, set] = createEngineState()
  const recovery = createRecoveryCoordinator((event, directory) => reduce(set, event, directory), 1)
  const result = deferred<unknown>()
  const actions = createActions(
    () => ({ session: { list: () => result.promise } }) as never,
    state,
    set,
    () => ({ url: "http://engine.test" }),
    recovery,
  )
  const loading = actions.loadSessions("C:/target")
  for (let index = 0; index < 100; index += 1)
    recovery.record(
      { type: "session.updated", properties: { info: session(`other-${index}`, { directory: "C:/other" }) } } as never,
      "C:/other",
    )
  result.resolve({ data: [session("target", { directory: "C:/target" })] })
  await loading

  const moved = {
    revision: 1,
    directory: "C:/source",
    event: {
      type: "session.next.moved",
      properties: { sessionID: "target", location: { directory: "C:/target" }, timestamp: 2 },
    } as Event,
  }
  expect(state.sessions.target.directory).toBe("C:/target")
  expect(eventInDirectory(moved, "C:/source")).toBeTrue()
  expect(eventInDirectory(moved, "C:/target")).toBeTrue()
  expect(eventInDirectory(moved, "C:/other")).toBeFalse()
})

test("pending older-page loads cannot recreate a deleted session", async () => {
  const [state, set] = createEngineState()
  const info = session("s1")
  set("sessions", "s1", info)
  set("loaded", "s1", true)
  set("transcripts", "s1", [{ info: { id: "new", sessionID: "s1", role: "user" }, parts: [] }] as never)
  set("cursors", "s1", "cursor")
  set("directory", "C:/work")
  const recovery = createRecoveryCoordinator((event, directory) => reduce(set, event, directory))
  const response = deferred<Response>()
  const originalFetch = globalThis.fetch
  globalThis.fetch = (() => response.promise) as typeof fetch

  try {
    const actions = createActions(
      () => ({}) as never,
      state,
      set,
      () => ({ url: "http://engine.test" }),
      recovery,
    )
    const loading = actions.loadOlder("s1")
    recovery.record({ type: "session.deleted", properties: { info } } as never, "C:/work")
    response.resolve(
      Response.json([{ info: { id: "old", sessionID: "s1", role: "user" }, parts: [] }], {
        headers: { "x-next-cursor": "older" },
      }),
    )

    expect(await loading).toBeFalse()
    expect(state.sessions.s1).toBeUndefined()
    expect(state.transcripts.s1).toBeUndefined()
    expect(state.cursors.s1).toBeUndefined()
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("a moved event for an unknown session replays after the complete snapshot", () => {
  const [state, set] = createEngineState()
  const recovery = createRecoveryCoordinator((event, directory) => reduce(set, event, directory))
  const token = recovery.begin()
  recovery.record(
    {
      type: "session.next.moved",
      properties: {
        sessionID: "s1",
        projectID: "moved-project",
        location: { directory: "C:/moved" },
        timestamp: 42,
      },
    } as never,
  )
  expect(state.sessions.s1).toBeUndefined()

  recovery.commit(token, (events) =>
    applySessionSnapshot(set, [session("s1")], () => true, events, recovery.replay),
  )

  expect(state.sessions.s1.directory).toBe("C:/moved")
  expect(state.sessions.s1.projectID).toBe("moved-project")
  expect(state.sessions.s1.time.updated).toBe(42)
})

test("a known session moved into a queried directory survives a stale snapshot omission", () => {
  const [state, set] = createEngineState()
  set("sessions", "s1", session("s1", { directory: "C:/source" }))
  const recovery = createRecoveryCoordinator((event, directory) => reduce(set, event, directory))
  const token = recovery.begin()
  recovery.record(
    {
      type: "session.next.moved",
      properties: { sessionID: "s1", location: { directory: "C:/target" }, timestamp: 42 },
    } as never,
    "C:/source",
  )

  recovery.commit(token, (events) =>
    applySessionSnapshot(
      set,
      [],
      (directory) => directory === "C:/target",
      events,
      recovery.replay,
    ),
  )

  expect(state.sessions.s1.directory).toBe("C:/target")
  expect(state.sessions.s1.time.updated).toBe(42)
})

test("stream exit invalidates generation requests before returning", async () => {
  const recovery = createRecoveryCoordinator(() => undefined)
  const request = recovery.begin(() => false)
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async () =>
    new Response(
      new ReadableStream<Uint8Array>({
        start(controller) {
          controller.close()
        },
      }),
    )) as typeof fetch

  try {
    await streamEvents(
      { url: "http://engine.test" },
      new AbortController().signal,
      () => undefined,
      100,
      recovery.advance,
    )
    expect(request.signal.aborted).toBeTrue()
    expect(recovery.current(request)).toBeFalse()
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("generation request timeouts abort their REST signal", async () => {
  const recovery = createRecoveryCoordinator(() => undefined)
  const request = recovery.begin(() => false, 5)
  await new Promise((resolve) => setTimeout(resolve, 10))
  expect(request.signal.aborted).toBeTrue()
  expect(recovery.current(request)).toBeFalse()
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
  expect(previousWorkspace.signal.aborted).toBeTrue()
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
