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
    { sessionId: "old-success", directory: "C:/reused" },
    { sessionId: "old-retry", directory: "C:/reused" },
    { sessionId: "old-missing", directory: "C:/reused" },
  ]
  const attempts: string[] = []
  const originalFetch = globalThis.fetch
  let retryFails = true
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init)
    const id = new URL(request.url).pathname.split("/").at(-1) ?? ""
    attempts.push(id)
    if (id === "old-retry" && retryFails) return Response.json({ message: "offline" }, { status: 503 })
    if (id === "old-missing") return new Response(null, { status: 404 })
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
    expect(attempts).toEqual(["old-success", "old-retry", "old-missing", "old-retry"])
    expect(attempts).not.toContain("new-session-at-reused-path")
  } finally {
    globalThis.fetch = originalFetch
  }
})
