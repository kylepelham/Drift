import { expect, test } from "bun:test"
import type { ToolPart } from "@opencode-ai/sdk/client"
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
    id: "launch",
    type: "tool",
    tool: "task",
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
      info: { id: "launch-message", sessionID: "parent", role: "assistant", time: { created: 0 } },
      parts: [part],
    },
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

function taskPart(id: string, output: string): ToolPart {
  return {
    id,
    type: "tool",
    tool: "task",
    sessionID: "parent",
    messageID: "message",
    callID: id,
    state: {
      status: "completed",
      input: { task_id: "child", description: id, subagent_type: "general" },
      output,
      title: id,
      metadata: { sessionId: "child" },
      time: { start: 1, end: 2 },
    },
  }
}

test("finished task cards do not follow a resumed child session's busy, retry, or error state", async () => {
  const { delegatedTaskStatus, delegatedTaskClickPolicy } = await import("../src/ui/parts")
  const { toolElapsedMs } = await import("../src/ui/tool-duration")
  const [state, set] = createEngineState()
  const original = taskPart("original", '<task id="child" state="completed">\n<task_result>First result</task_result>\n</task>')
  const resumed: ToolPart = {
    ...taskPart("resumed", ""),
    state: { status: "running", input: { task_id: "child" }, time: { start: 3 } },
  }
  for (const type of ["busy", "retry", "idle"] as const) {
    set("status", "child", type === "retry" ? { type, attempt: 1, message: "retry", next: 10 } : { type })
    const previous = delegatedTaskStatus(state, original, "child")
    const current = delegatedTaskStatus(state, resumed, "child")
    expect(previous).toBe("completed")
    expect(current).toBe("running")
    expect(delegatedTaskClickPolicy(previous, "child")).toBe("expand")
    expect(delegatedTaskClickPolicy(current, "child")).toBe("navigate")
    expect(toolElapsedMs(original.state, 100)).toBe(1)
    expect(toolElapsedMs(resumed.state, 100)).toBe(97)
  }
  set("errors", "child", "The resumed invocation failed")
  expect(delegatedTaskStatus(state, original, "child")).toBe("completed")
  expect(delegatedTaskStatus(state, resumed, "child")).toBe("error")
})

test("failed task cards stay failed when the child is resumed or later completes", async () => {
  const { delegatedTaskStatus } = await import("../src/ui/parts")
  const [state, set] = createEngineState()
  const failed: ToolPart = {
    ...taskPart("failed", ""),
    state: { status: "error", input: {}, error: "Original failure", time: { start: 1, end: 2 } },
  }
  set("status", "child", { type: "busy" })
  expect(delegatedTaskStatus(state, failed, "child")).toBe("error")
  expect(delegatedTaskStatus(state, taskPart("reported-error", '<task id="child" state="error">'), "child")).toBe("error")
  set("status", "child", { type: "idle" })
  expect(delegatedTaskStatus(state, failed, "child")).toBe("error")
})

test("legacy foreground results stay completed without a loaded parent transcript", async () => {
  const { delegatedTaskStatus } = await import("../src/ui/parts")
  const [state, set] = createEngineState()
  set("status", "child", { type: "busy" })
  const legacy = taskPart("legacy", "task_id: child\n<task_result>Already finished</task_result>")
  expect(delegatedTaskStatus(state, legacy, "child")).toBe("completed")
})

test("background admission and spawned-thread receipts are not finished-work results", async () => {
  const { delegatedTaskStatus } = await import("../src/ui/parts")
  const [state, set] = createEngineState()
  const background = taskPart("background", "Background task started")
  if (background.state.status === "completed") background.state.metadata.background = true
  const spawned = {
    ...taskPart("spawned", 'Spawned thread "Child" (id child); its seed prompt was accepted for processing.'),
    tool: "spawn_thread",
  }
  for (const type of ["busy", "idle"] as const) {
    set("status", "child", { type })
    expect(delegatedTaskStatus(state, background, "child")).toBe("running")
    expect(delegatedTaskStatus(state, spawned, "child")).toBe("running")
  }
})

test("background completions belong to the invocation preceding them, including after reload", async () => {
  const { delegatedTaskStatus } = await import("../src/ui/parts")
  const original = taskPart("original", '<task id="child" state="running">')
  const resumed = taskPart("resumed", '<task id="child" state="running">')
  const notification = (id: string, status: string) => ({
    id, type: "text" as const, sessionID: "parent", messageID: "message", synthetic: true,
    text: `<task id="child" state="${status}">`,
  })
  const parts = [original, notification("first-result", "completed"), resumed]
  // Fresh stores exercise persisted history, not a component-local cache of the old result.
  for (const childStatus of ["busy", "retry", "idle"] as const) {
    const [state, set] = createEngineState()
    set("transcripts", "parent", [{
      info: { id: "message", sessionID: "parent", role: "assistant", time: { created: 1 } },
      parts,
    }] as never)
    set("status", "child", childStatus === "retry"
      ? { type: childStatus, attempt: 1, message: "retry", next: 10 }
      : { type: childStatus })
    expect(delegatedTaskStatus(state, original, "child")).toBe("completed")
    expect(delegatedTaskStatus(state, resumed, "child")).toBe("running")
    set("transcripts", "parent", 0, "parts", [...parts, notification("second-result", "error")])
    expect(delegatedTaskStatus(state, original, "child")).toBe("completed")
    expect(delegatedTaskStatus(state, resumed, "child")).toBe("error")
  }
})

test("a result from an earlier invocation cannot settle a detached background card", async () => {
  const { delegatedTaskStatus } = await import("../src/ui/parts")
  const [state, set] = createEngineState()
  set("transcripts", "parent", [{
    info: { id: "old", sessionID: "parent", role: "assistant", time: { created: 1 } },
    parts: [taskPart("old", '<task id="child" state="completed">')],
  }] as never)
  expect(delegatedTaskStatus(state, taskPart("new", '<task id="child" state="running">'), "child")).toBe("running")
})

test("background cards ignore another invocation's tool output while awaiting their own notification", async () => {
  const { delegatedTaskStatus } = await import("../src/ui/parts")
  const [state, set] = createEngineState()
  const background = taskPart("background", '<task id="child" state="running">')
  const later = taskPart("later", '<task id="child" state="completed">')
  set("transcripts", "parent", [{
    info: { id: "message", sessionID: "parent", role: "assistant", time: { created: 1 } },
    parts: [background, later],
  }] as never)
  expect(delegatedTaskStatus(state, background, "child")).toBe("running")
  expect(delegatedTaskStatus(state, later, "child")).toBe("completed")
  set("transcripts", "parent", 0, "parts", [background, later, {
    id: "notification", type: "text", sessionID: "parent", messageID: "message", synthetic: true,
    text: '<task id="child" state="error">',
  }])
  expect(delegatedTaskStatus(state, background, "child")).toBe("error")
  expect(delegatedTaskStatus(state, later, "child")).toBe("completed")
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
