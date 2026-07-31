import { afterEach, expect, test } from "bun:test"
import type { Permission } from "@opencode-ai/sdk/client"
import { createActions } from "../src/engine/actions"
import { createEngineState } from "../src/engine/store"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

const originalFetch = globalThis.fetch
afterEach(() => {
  globalThis.fetch = originalFetch
})

const home = "C:/one"
const elsewhere = "C:/two"

function permission(): Permission {
  return {
    id: "p1",
    type: "bash",
    sessionID: "s1",
    messageID: "m1",
    title: "rm -rf",
    metadata: { directory: elsewhere },
    time: { created: 1 },
  } as Permission
}

// The engine keeps reporting the ask until it is actually answered, which is what makes a
// swallowed reply failure permanent: the card is gone so reconciliation never revisits it.
function harness(reply: () => Promise<Response>) {
  const [state, set] = createEngineState()
  set("directory", home)
  set("permissions", "s1", [permission()])
  const pending = { value: true }
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init)
    const url = new URL(request.url)
    if (request.method === "POST") return reply()
    if (url.pathname === "/permission")
      return Response.json(
        pending.value ? [{ id: "p1", sessionID: "s1", permission: "bash", metadata: { title: "rm -rf" } }] : [],
      )
    return Response.json([])
  }) as typeof fetch
  const actions = createActions(
    () => ({}) as never,
    state,
    set,
    () => ({ url: "http://engine.test" }),
  )
  return { state, actions, pending }
}

test("a rejected cross-workspace reply keeps the ask visible and retryable", async () => {
  const { state, actions } = harness(() => Promise.reject(new Error("network down")))
  expect(await actions.replyPermission("s1", "p1", "once")).toBe(false)
  expect(state.permissions.s1?.map((item) => item.id)).toEqual(["p1"])
  expect(state.notices.at(-1)?.variant).toBe("error")
  await actions.refreshPermissions([elsewhere])
  expect(state.permissions.s1?.map((item) => item.id)).toEqual(["p1"])
})

test("a 500 from a cross-workspace reply keeps the ask visible and retryable", async () => {
  const { state, actions } = harness(async () => new Response("boom", { status: 500 }))
  expect(await actions.replyPermission("s1", "p1", "once")).toBe(false)
  expect(state.permissions.s1?.map((item) => item.id)).toEqual(["p1"])
  expect(state.notices.at(-1)?.variant).toBe("error")
  await actions.refreshPermissions([elsewhere])
  expect(state.permissions.s1?.map((item) => item.id)).toEqual(["p1"])
})

// UI callers fire-and-forget this action, so an offline client must surface a failure
// rather than reject and silently leave the card in place with no explanation.
test("a local reply that throws while offline keeps the ask visible and reports it", async () => {
  const [state, set] = createEngineState()
  set("directory", home)
  set("permissions", "s1", [{ ...permission(), metadata: { directory: home } } as Permission])
  const actions = createActions(
    () => {
      throw new Error("engine offline")
    },
    state,
    set,
    () => ({ url: "http://engine.test" }),
  )
  expect(await actions.replyPermission("s1", "p1", "once")).toBe(false)
  expect(state.permissions.s1?.map((item) => item.id)).toEqual(["p1"])
  expect(state.notices.at(-1)?.variant).toBe("error")
})

test("a confirmed cross-workspace reply clears the ask and survives a racing poll", async () => {
  const { state, actions, pending } = harness(async () => new Response(null, { status: 200 }))
  expect(await actions.replyPermission("s1", "p1", "once")).toBe(true)
  expect(state.permissions.s1 ?? []).toHaveLength(0)
  expect(state.notices).toHaveLength(0)
  await actions.refreshPermissions([elsewhere])
  expect(state.permissions.s1 ?? []).toHaveLength(0)
  pending.value = false
  await actions.refreshPermissions([elsewhere])
  expect(state.permissions.s1 ?? []).toHaveLength(0)
})

test("concurrent replies to one permission share the same engine request", async () => {
  let requests = 0
  let release!: () => void
  const pending = new Promise<void>((resolve) => (release = resolve))
  const { state, actions } = harness(async () => {
    requests += 1
    await pending
    return new Response(null, { status: 200 })
  })

  const first = actions.replyPermission("s1", "p1", "once")
  const second = actions.replyPermission("s1", "p1", "once")
  expect(first).toBe(second)
  expect(requests).toBe(1)
  release()
  expect(await Promise.all([first, second])).toEqual([true, true])
  expect(state.permissions.s1 ?? []).toHaveLength(0)
  expect(state.notices).toHaveLength(0)
})

test("v2 permission replies use the protocol endpoint and payload", async () => {
  const [state, set] = createEngineState()
  set("directory", home)
  set("permissions", "s1", [{ ...permission(), metadata: { directory: home }, driftProtocol: "v2" } as Permission])
  let request!: Request
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    request = input instanceof Request ? input : new Request(input, init)
    return new Response(null, { status: 204 })
  }) as typeof fetch
  const actions = createActions(
    () => ({}) as never,
    state,
    set,
    () => ({ url: "http://engine.test" }),
  )

  expect(await actions.replyPermission("s1", "p1", "always")).toBeTrue()
  expect(new URL(request.url).pathname).toBe("/api/session/s1/permission/p1/reply")
  expect(await request.json()).toEqual({ reply: "always" })
})
