import { expect, test } from "bun:test"
import { createActions, type PendingSessionDeletion } from "../src/engine/actions"
import { createEngineState } from "../src/engine/store"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

test("pending deletion confirms successes and retries only failed exact IDs", async () => {
  const [state, set] = createEngineState()
  const entries: PendingSessionDeletion[] = [
    { sessionId: "old-success", directory: "C:/reused", claim: "claim-success" },
    { sessionId: "old-retry", directory: "C:/reused", claim: "claim-retry" },
    { sessionId: "old-missing", directory: "C:/reused", claim: "claim-missing" },
  ]
  const attempts: string[] = []
  const originalFetch = globalThis.fetch
  let retryFails = true
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init)
    const id = new URL(request.url).pathname.split("/").at(-1) ?? ""
    attempts.push(`${request.method}:${id}`)
    if (id === "old-retry" && retryFails) return Response.json({ message: "offline" }, { status: 503 })
    if (id === "old-missing") return new Response(null, { status: 404 })
    if (request.method === "GET") return new Response(null, { status: 404 })
    return Response.json(true)
  }) as typeof fetch

  try {
    const actions = createActions(
      () => ({}) as never,
      state,
      set,
      () => ({ url: "http://engine.test" }),
    )
    expect(await actions.removePendingSessions(entries)).toEqual({
      confirmed: [entries[0], entries[2]],
      retry: [entries[1]],
    })
    retryFails = false
    expect(await actions.removePendingSessions([entries[1]])).toEqual({ confirmed: [entries[1]], retry: [] })
    expect(attempts).toEqual([
      "DELETE:old-success",
      "GET:old-success",
      "DELETE:old-retry",
      "DELETE:old-missing",
      "DELETE:old-retry",
      "GET:old-retry",
    ])
    expect(attempts).not.toContain("DELETE:new-session-at-reused-path")
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("pending deletion keeps its claim when a successful delete did not remove the exact session", async () => {
  const [state, set] = createEngineState()
  const entry = { sessionId: "still-present", directory: "C:/reused", claim: "opaque" }
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init)
    return request.method === "DELETE" ? Response.json(true) : Response.json({ id: entry.sessionId })
  }) as typeof fetch
  try {
    const actions = createActions(
      () => ({}) as never,
      state,
      set,
      () => ({ url: "http://engine.test" }),
    )
    expect(await actions.removePendingSessions([entry])).toEqual({ confirmed: [], retry: [] })
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("a hung deletion request resolves the sweep without releasing the claim, then confirms on a later sweep", async () => {
  const [state, set] = createEngineState()
  const entry = { sessionId: "hung", directory: "C:/reused", claim: "opaque" }
  const originalFetch = globalThis.fetch
  let hang = true
  const aborted: string[] = []
  globalThis.fetch = ((input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init)
    if (!hang) return Promise.resolve(new Response(null, { status: 404 }))
    // Never settles on its own; only the caller's deadline can abort it.
    return new Promise<Response>((_resolve, reject) => {
      init?.signal?.addEventListener("abort", () => {
        aborted.push(request.method)
        reject(new DOMException("Aborted", "AbortError"))
      })
    })
  }) as typeof fetch

  try {
    const actions = createActions(
      () => ({}) as never,
      state,
      set,
      () => ({ url: "http://engine.test" }),
    )
    const hungSweep = actions.removePendingSessions([entry], { request: 25, sweep: 5000 })
    // The sweep must settle on its own deadline rather than hanging until the app restarts.
    expect(await hungSweep).toEqual({ confirmed: [], retry: [] })
    expect(aborted).toEqual(["DELETE"])

    hang = false
    expect(await actions.removePendingSessions([entry])).toEqual({ confirmed: [entry], retry: [] })
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("an exhausted sweep budget releases entries it never sent to the engine", async () => {
  const [state, set] = createEngineState()
  const entries: PendingSessionDeletion[] = [
    { sessionId: "attempted", directory: "C:/reused", claim: "claim-attempted" },
    { sessionId: "untouched", directory: "C:/reused", claim: "claim-untouched" },
  ]
  const attempted: string[] = []
  const originalFetch = globalThis.fetch
  globalThis.fetch = ((input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init)
    attempted.push(new URL(request.url).pathname.split("/").at(-1) ?? "")
    return new Promise<Response>((_resolve, reject) => {
      init?.signal?.addEventListener("abort", () => reject(new DOMException("Aborted", "AbortError")))
    })
  }) as typeof fetch

  try {
    const actions = createActions(
      () => ({}) as never,
      state,
      set,
      () => ({ url: "http://engine.test" }),
    )
    // The first entry burns the whole budget, so the second is never sent and is safe to unclaim.
    expect(await actions.removePendingSessions(entries, { request: 30, sweep: 20 })).toEqual({
      confirmed: [],
      retry: [entries[1]],
    })
    expect(attempted).toEqual(["attempted"])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("workspace purge session discovery follows every experimental session cursor", async () => {
  const [state, set] = createEngineState()
  const cursors: [string | null, string | null][] = []
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const url = new URL(String(input))
    const cursor = url.searchParams.get("cursor")
    const cursorID = url.searchParams.get("cursorID")
    cursors.push([cursor, cursorID])
    if (!cursor)
      return Response.json([{ id: "newer" }], {
        headers: { "x-next-cursor": "200", "x-next-cursor-id": "newer" },
      })
    if (cursor === "200")
      return Response.json([{ id: "older" }], {
        headers: { "x-next-cursor": "100", "x-next-cursor-id": "older" },
      })
    return Response.json([{ id: "oldest" }])
  }) as typeof fetch
  try {
    const actions = createActions(
      () => ({}) as never,
      state,
      set,
      () => ({ url: "http://engine.test" }),
    )
    expect(await actions.sessionIdsAt("C:/purge")).toEqual(["newer", "older", "oldest"])
    expect(cursors).toEqual([
      [null, null],
      ["200", "newer"],
      ["100", "older"],
    ])
  } finally {
    globalThis.fetch = originalFetch
  }
})
