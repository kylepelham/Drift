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
    expect(await actions.removePendingSessions(entries)).toEqual([entries[0], entries[2]])
    retryFails = false
    expect(await actions.removePendingSessions([entries[1]])).toEqual([entries[1]])
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
    expect(await actions.removePendingSessions([entry])).toEqual([])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("workspace purge session discovery follows every experimental session cursor", async () => {
  const [state, set] = createEngineState()
  const cursors: (string | null)[] = []
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const url = new URL(String(input))
    const cursor = url.searchParams.get("cursor")
    cursors.push(cursor)
    if (!cursor)
      return Response.json([{ id: "newer" }], { headers: { "x-next-cursor": "200" } })
    if (cursor === "200")
      return Response.json([{ id: "older" }], { headers: { "x-next-cursor": "100" } })
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
    expect(cursors).toEqual([null, "200", "100"])
  } finally {
    globalThis.fetch = originalFetch
  }
})
