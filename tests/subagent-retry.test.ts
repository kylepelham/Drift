import { expect, test } from "bun:test"
import { createEngineState } from "../src/engine/store"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

test("retry model switching targets the exact parked attempt through the experimental endpoint", async () => {
  const { createActions } = await import("../src/engine/actions")
  const [state, set] = createEngineState()
  set("sessions", "ses", { id: "ses", directory: "D:/work/beta" } as never)
  const originalFetch = globalThis.fetch
  let request: Request | undefined
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    request = input instanceof Request ? input : new Request(input, init)
    return new Response(null, { status: 204 })
  }) as typeof fetch
  try {
    const actions = createActions(
      () => {
        throw new Error("the SDK client is not involved in retry switching")
      },
      state,
      set,
      () => ({ url: "http://engine.test" }),
    )
    expect(
      await actions.switchRetryModel("ses", "msg_1", { providerID: "openai", modelID: "gpt-5" }, "high"),
    ).toEqual({ ok: true })
    expect(request!.method).toBe("PUT")
    expect(new URL(request!.url).pathname).toBe("/experimental/session/ses/retry-model")
    expect(await request!.json()).toEqual({
      messageID: "msg_1",
      providerID: "openai",
      modelID: "gpt-5",
      variant: "high",
    })

    globalThis.fetch = (async () =>
      Response.json({ data: { message: "Session is not waiting to retry" } }, { status: 409 })) as typeof fetch
    expect(await actions.switchRetryModel("ses", "msg_1", { providerID: "openai", modelID: "gpt-5" })).toEqual({
      ok: false,
      error: "Session is not waiting to retry",
    })

    expect(await actions.switchRetryModel("gone", "msg_1", { providerID: "openai", modelID: "gpt-5" })).toEqual({
      ok: false,
      error: "The retrying session is no longer available",
    })
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("parent delegated status follows ordinary child errors, resumed work, and completion", async () => {
  const { delegatedTaskStatus } = await import("../src/ui/parts")
  const [state, set] = createEngineState()
  const part = {
    sessionID: "parent",
    state: { status: "completed", input: {}, output: '<task id="child" state="running">' },
  } as never
  set("errors", "child", "usage limit reached")
  expect(delegatedTaskStatus(state, part, "child")).toBe("error")
  set("errors", "child", undefined!)
  set("status", "child", { type: "busy" })
  expect(delegatedTaskStatus(state, part, "child")).toBe("running")
  set("status", "child", { type: "idle" })
  set("transcripts", "parent", [
    {
      info: { id: "done", sessionID: "parent", role: "assistant", time: { created: 1 } },
      parts: [{ id: "result", type: "text", text: '<task id="child" state="completed">', sessionID: "parent", messageID: "done" }],
    },
  ] as never)
  expect(delegatedTaskStatus(state, part, "child")).toBe("completed")
})

test("a live delegated part overrides an older completion marker", async () => {
  const { delegatedTaskClickPolicy, delegatedTaskStatus } = await import("../src/ui/parts")
  const [state, set] = createEngineState()
  set("transcripts", "parent", [
    {
      info: { id: "old", sessionID: "parent", role: "assistant", time: { created: 1 } },
      parts: [{ id: "result", type: "text", text: '<task id="child" state="completed">', sessionID: "parent", messageID: "old" }],
    },
  ] as never)
  const live = { sessionID: "parent", state: { status: "running", input: {}, time: { start: 1 } } } as never
  const status = delegatedTaskStatus(state, live, "child")
  expect(status).toBe("running")
  expect(delegatedTaskClickPolicy(status, "child")).toBe("navigate")
})

test("a running delegated row recovers its child when parallel task metadata is missing", async () => {
  const { delegatedChildId, delegatedTaskClickPolicy, delegatedTaskStatus } = await import("../src/ui/parts")
  const [state, set] = createEngineState()
  set("sessions", "child", {
    id: "child",
    parentID: "parent",
    title: "Explore service sinks (@explore subagent)",
    time: { created: 1, updated: 1 },
  } as never)
  const part = {
    tool: "task",
    sessionID: "parent",
    state: {
      status: "running",
      input: { description: "Explore service sinks", subagent_type: "explore" },
      time: { start: 2 },
    },
  } as never
  const childId = delegatedChildId(state, part)
  const status = delegatedTaskStatus(state, part, childId!)
  expect(childId).toBe("child")
  expect(status).toBe("running")
  expect(delegatedTaskClickPolicy(status, childId)).toBe("navigate")

  set("sessions", "duplicate", {
    id: "duplicate",
    parentID: "parent",
    title: "Explore service sinks (@explore subagent)",
    time: { created: 2, updated: 2 },
  } as never)
  expect(delegatedChildId(state, part)).toBeNull()
})

test("running delegated rows navigate while terminal rows expand without lifecycle badges", async () => {
  const { delegatedTaskClickPolicy } = await import("../src/ui/parts")
  expect(delegatedTaskClickPolicy("running", "child")).toBe("navigate")
  expect(delegatedTaskClickPolicy("completed", "child")).toBe("expand")
  expect(delegatedTaskClickPolicy("error", "child")).toBe("expand")
  const source = await Bun.file("src/ui/parts.tsx").text()
  expect(source).toContain("selectSession(spawnedId()!)")
  expect(source).toContain("function spawnedId()")
  expect(source).not.toContain("const spawnedId =")
})
