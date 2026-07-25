import { expect, test } from "bun:test"
import type { Session } from "@opencode-ai/sdk/client"
import { createActions } from "../src/engine/actions"
import { createEngineState, putSessions } from "../src/engine/store"

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

test("bulk session hydration preserves updates and clears dropped optional fields", () => {
  const [state, set] = createEngineState()
  putSessions(set, [session("one", { revert: { messageID: "m1" }, share: { url: "https://example.test" } })])
  putSessions(set, [session("one", { title: "updated" }), session("two")])

  expect(Object.keys(state.sessions)).toEqual(["one", "two"])
  expect(state.sessions.one.title).toBe("updated")
  expect(state.sessions.one.revert).toBeUndefined()
  expect(state.sessions.one.share).toBeUndefined()
})

test("concurrent global session loads share one request", async () => {
  const [state, set] = createEngineState()
  const originalFetch = globalThis.fetch
  let requests = 0
  let release!: () => void
  const pending = new Promise<void>((resolve) => (release = resolve))
  globalThis.fetch = (async () => {
    requests += 1
    await pending
    return Response.json([session("one"), session("two")])
  }) as typeof fetch

  try {
    const actions = createActions(
      () => ({}) as never,
      state,
      set,
      () => ({ url: "http://engine.test" }),
    )
    const first = actions.loadAllSessions()
    const second = actions.loadAllSessions()
    release()
    await Promise.all([first, second])

    expect(requests).toBe(1)
    expect(Object.keys(state.sessions)).toEqual(["one", "two"])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("startup splash waits for workspace bootstrap without trapping empty or failed startup", async () => {
  const { startupReady } = await import("../src/ui/startup")
  const input = {
    workspacesReady: false,
    workspacePath: "C:/work",
    connection: "connecting" as const,
    bootstrappedDirectory: "",
    startupError: "",
  }

  expect(startupReady(input)).toBeFalse()
  expect(startupReady({ ...input, workspacesReady: true, workspacePath: null })).toBeTrue()
  expect(startupReady({ ...input, startupError: "engine failed" })).toBeTrue()
  expect(startupReady({ ...input, workspacesReady: true, connection: "online" })).toBeFalse()
  expect(
    startupReady({
      ...input,
      workspacesReady: true,
      connection: "online",
      bootstrappedDirectory: "C:/work",
    }),
  ).toBeTrue()
})
