import { expect, test } from "bun:test"
import type { Event } from "@opencode-ai/sdk/client"
import { reduce } from "../src/engine/events"
import { createEngineState } from "../src/engine/store"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

const toolEvent = (partId: string, tool: string, status: string): Event =>
  ({
    type: "message.part.updated",
    properties: {
      part: { id: partId, sessionID: "child", messageID: "m1", type: "tool", tool, state: { status } },
    },
  }) as unknown as Event

test("session.updated clears revert and share keys the engine dropped", () => {
  const [state, set] = createEngineState()
  const updated = (info: Record<string, unknown>): Event =>
    ({ type: "session.updated", properties: { info } }) as unknown as Event
  reduce(set, updated({ id: "s1", title: "t", revert: { messageID: "m5" }, share: { url: "u" } }))
  expect(state.sessions["s1"].revert?.messageID).toBe("m5")
  reduce(set, updated({ id: "s1", title: "t" }))
  expect(state.sessions["s1"].revert).toBeUndefined()
  expect(state.sessions["s1"].share).toBeUndefined()
})

test("fixEscapedEmphasis lets path-ending emphasis close without touching escapes or code", async () => {
  const { fixEscapedEmphasis } = await import("../src/ui/markdown")
  expect(fixEscapedEmphasis("*C:\\* (30 entries)")).toBe("*C:\\\\* (30 entries)")
  expect(fixEscapedEmphasis("**S:\\Personal\\Drift\\** done")).toBe("**S:\\Personal\\Drift\\\\** done")
  expect(fixEscapedEmphasis("`**C:\\**` and ```\nS:\\**\n```")).toBe("`**C:\\**` and ```\nS:\\**\n```")
  expect(fixEscapedEmphasis("literal \\*star\\* stays and 5 \\* 3")).toBe("literal \\*star\\* stays and 5 \\* 3")
})

test("progressive code chunks retain the complete file", async () => {
  const { codeChunks } = await import("../src/ui/markdown")
  const code = Array.from({ length: 401 }, (_, index) => `line ${index + 1}`).join("\n")
  expect(codeChunks(code).length).toBe(3)
  expect(codeChunks(code).join("\n")).toBe(code)
})

test("taskBody extracts prompt and task_result for task cards", async () => {
  const { taskBody } = await import("../src/ui/parts")
  const part = (tool: string, input: Record<string, string>, output: string) =>
    ({ tool, state: { status: "completed", input, output } }) as never
  expect(taskBody(part("task", { prompt: "do x" }, "<task id=\"s1\" state=\"completed\">\n<task_result>\nall done\n</task_result>\n</task>"))).toEqual({
    prompt: "do x",
    result: "all done",
  })
  expect(taskBody(part("spawn_thread", { task: "spin off" }, "Spawned thread ok"))).toEqual({
    prompt: "spin off",
    result: "Spawned thread ok",
  })
  expect(taskBody(part("bash", {}, "x"))).toBeNull()
})

test("task headings retain the agent and task title", async () => {
  const { taskHeading } = await import("../src/ui/parts")
  expect(taskHeading("explore", "Map settings translations")).toBe("Explore Map settings translations")
  expect(taskHeading("general")).toBe("General")
})

test("compaction-only user messages retain their delimiter part", async () => {
  const { compactionParts } = await import("../src/ui/message")
  const entry = {
    info: { id: "m1", role: "user", sessionID: "s1" },
    parts: [{ id: "p1", messageID: "m1", sessionID: "s1", type: "compaction", auto: true }],
  } as never
  expect(compactionParts(entry).map((part) => part.id)).toEqual(["p1"])
})

test("compaction boundary merges into its adjacent summary", async () => {
  const { mergeCompactionEntries } = await import("../src/ui/chat")
  const boundary = {
    info: { id: "u1", role: "user", sessionID: "s1" },
    parts: [{ id: "p1", messageID: "u1", sessionID: "s1", type: "compaction", auto: true }],
  }
  const summary = {
    info: { id: "a1", role: "assistant", sessionID: "s1", parentID: "u1", summary: true },
    parts: [{ id: "p2", messageID: "a1", sessionID: "s1", type: "text", text: "summary" }],
  }
  expect(mergeCompactionEntries([boundary, summary] as never).map((entry) => entry.info.id)).toEqual(["a1"])
  expect(mergeCompactionEntries([boundary] as never).map((entry) => entry.info.id)).toEqual(["u1"])
})

test("successful compaction clears a transient session error", () => {
  const [state, set] = createEngineState()
  set("errors", "s1", "Your input exceeds the context window")
  reduce(
    set,
    { type: "session.compacted", properties: { sessionID: "s1" } } as unknown as Event,
  )
  expect(state.errors["s1"]).toBeUndefined()
})

test("sidebar drag converts screen movement through the current zoom scale", async () => {
  const { sidebarWidthFromDrag } = await import("../src/ui/sidebar")
  expect(sidebarWidthFromDrag(256, 30, 1.5)).toBe(276)
  expect(sidebarWidthFromDrag(470, 30, 1)).toBe(480)
})

test("fixed menus convert visual coordinates and viewport bounds through CSS zoom", async () => {
  const { fixedMenuPosition } = await import("../src/state/zoom")
  const metrics = { scale: 1.5, viewportWidth: 1200, viewportHeight: 900 }
  expect(fixedMenuPosition(300, 225, 200, 100, metrics)).toEqual({ left: 200, top: 150, viewportHeight: 600 })
  expect(fixedMenuPosition(1170, 870, 200, 100, metrics)).toEqual({ left: 592, top: 492, viewportHeight: 600 })
})

test("provider credentials dispose cached instances and refresh connection state", async () => {
  const { createActions } = await import("../src/engine/actions")
  const [state, set] = createEngineState()
  set("directory", "C:\\repo")
  const requests: string[] = []
  const providerLists = [["opencode"], []]
  const client = {
    auth: {
      set: async () => ({ data: true }),
    },
    provider: {
      list: async () => ({
        data: { all: [], connected: providerLists.shift() ?? [], default: {} },
      }),
    },
  }
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init)
    requests.push(`${request.method} ${new URL(request.url).pathname}`)
    return new Response("true", { status: 200, headers: { "content-type": "application/json" } })
  }) as typeof fetch

  try {
    const actions = createActions(
      () => client as never,
      state,
      set,
      () => ({ url: "http://engine.test" }),
    )
    expect(await actions.setProviderKey("opencode", "test-key")).toEqual({ ok: true, connected: true })
    expect(state.connected).toEqual(["opencode"])
    expect(await actions.disconnectProvider("opencode")).toEqual({ ok: true, connected: false })
    expect(state.connected).toEqual([])
    expect(requests).toEqual([
      "POST /global/dispose",
      "DELETE /auth/opencode",
      "POST /global/dispose",
    ])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("upward transcript gestures unstick immediately near the bottom", async () => {
  const { accumulatedWheelTarget, normalizedWheelDelta, scrollGestureSticks } = await import("../src/ui/chat")
  expect(scrollGestureSticks(1000, 980, 20)).toBeFalse()
  expect(scrollGestureSticks(980, 1000, 20)).toBeTrue()
  expect(scrollGestureSticks(980, 1000, 120)).toBeFalse()
  expect(normalizedWheelDelta(3, 0, 800)).toBe(3)
  expect(normalizedWheelDelta(3, 1, 800)).toBe(48)
  expect(normalizedWheelDelta(2, 2, 800)).toBe(1600)
  expect(accumulatedWheelTarget(100, null, 40, 500)).toBe(140)
  expect(accumulatedWheelTarget(105, 140, 40, 500)).toBe(180)
  expect(accumulatedWheelTarget(490, null, 40, 500)).toBe(500)
})

test("tall row measurement only compensates rows actually above the viewport", async () => {
  const { resizeCompensation } = await import("../src/ui/chat")
  expect(resizeCompensation(96, 2000, 2100, 1000)).toBe(0)
  expect(resizeCompensation(96, 2000, 900, 1000)).toBe(1904)
})

test("thinking follows OpenCode's active user turn", async () => {
  const { thinkingAfterMessage } = await import("../src/ui/chat")
  const message = (id: string, role: "user" | "assistant", parentID?: string, completed?: number) =>
    ({ info: { id, role, parentID, time: { created: 1, completed } }, parts: [] }) as never
  const first = message("u1", "user")
  const response = message("a1", "assistant", "u1")
  const steer = message("u2", "user")
  expect(thinkingAfterMessage([first, response, steer], "busy")).toBe("a1")
  response.info.time.completed = 2
  expect(thinkingAfterMessage([first, response, steer], "busy")).toBe("u2")
  const steeredResponse = message("a2", "assistant", "u2")
  expect(thinkingAfterMessage([first, response, steer, steeredResponse], "busy")).toBe("a2")
  expect(thinkingAfterMessage([first, response, steer, steeredResponse], "retry")).toBe("a2")
  expect(thinkingAfterMessage([first, response, steer, steeredResponse], "idle")).toBeNull()
})

test("thinking derives the first provider reasoning heading for the active turn", async () => {
  const { reasoningHeading, thinkingState } = await import("../src/ui/chat")
  expect(reasoningHeading("## Inspecting `events.ts` ##\n\nChecking the reducer.")).toBe("Inspecting events.ts")
  expect(reasoningHeading("<h3>Comparing <em>providers</em></h3>")).toBe("Comparing providers")
  expect(reasoningHeading("**Reading [OpenCode](https://opencode.ai) behavior**\n\nDetails")).toBe("Reading OpenCode behavior")
  expect(reasoningHeading("Unformatted reasoning text")).toBeUndefined()

  const user = { info: { id: "u1", role: "user", time: { created: 1 } }, parts: [] }
  const assistant = {
    info: { id: "a1", role: "assistant", parentID: "u1", time: { created: 2 } },
    parts: [{ id: "p1", type: "reasoning", text: "**Tracing session state**", time: { start: 2 } }],
  }
  expect(thinkingState([user, assistant] as never, "busy")).toEqual({
    messageID: "a1",
    heading: "Tracing session state",
  })
})

test("retry presentation follows OpenCode countdown and truncation", async () => {
  const { retryPresentation } = await import("../src/ui/chat")
  const status = { type: "retry", attempt: 3, message: "x".repeat(90), next: 15_000 } as const
  expect(retryPresentation(status, 7_400)).toEqual({
    message: "x".repeat(80) + "...",
    truncated: true,
    info: "Retrying in 8s - attempt #3",
  })
  expect(retryPresentation({ ...status, message: "Rate limited" }, 16_000).info).toBe("Retrying - attempt #3")
})

test("busy thinking is suppressed by an assistant error while retry remains visible", async () => {
  const { thinkingState } = await import("../src/ui/chat")
  const entries = [
    { info: { id: "u1", role: "user", time: { created: 1 } }, parts: [] },
    {
      info: {
        id: "a1",
        role: "assistant",
        parentID: "u1",
        time: { created: 2 },
        error: { name: "APIError", data: { message: "failed" } },
      },
      parts: [],
    },
  ] as never
  expect(thinkingState(entries, "busy")).toBeNull()
  expect(thinkingState(entries, "retry")?.messageID).toBe("a1")
})

test("assistant errors unwrap provider JSON and preserve plain text", async () => {
  const { errorText, unwrapErrorMessage } = await import("../src/ui/message")
  expect(unwrapErrorMessage('Error: {"error":{"type":"rate_limit","message":"slow down"}}')).toBe(
    "rate_limit: slow down",
  )
  expect(unwrapErrorMessage('prefix {"message":"credit balance is too low"} suffix')).toBe(
    "credit balance is too low",
  )
  expect(errorText({ name: "ProviderError", data: { message: "plain failure" } })).toBe("plain failure")
})

test("message part deltas accumulate streamed reasoning summaries", () => {
  const [state, set] = createEngineState()
  set("loaded", "s1", true)
  set("transcripts", "s1", [
    {
      info: { id: "a1", sessionID: "s1", role: "assistant", time: { created: 1 } },
      parts: [{ id: "p1", sessionID: "s1", messageID: "a1", type: "reasoning", text: "**Tracing" }],
    },
  ] as never)
  reduce(
    set,
    {
      type: "message.part.delta",
      properties: { sessionID: "s1", messageID: "a1", partID: "p1", field: "text", delta: " events**" },
    } as never,
  )
  expect((state.transcripts.s1[0].parts[0] as { text: string }).text).toBe("**Tracing events**")
})

test("context usage skips a trailing zero-token assistant message", async () => {
  const { contextStats } = await import("../src/engine/store")
  const [state, set] = createEngineState()
  const assistant = (id: string, total: number) => ({
    info: {
      id,
      sessionID: "s1",
      role: "assistant",
      providerID: "openai",
      modelID: "gpt-5",
      tokens: { total, input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
    },
    parts: [],
  })
  set("transcripts", "s1", [assistant("a1", 50_000), assistant("a2", 0)] as never)
  set("providers", [
    {
      id: "openai",
      name: "OpenAI",
      models: { "gpt-5": { id: "gpt-5", limit: { context: 100_000 } } },
    },
  ] as never)
  expect(contextStats(state, "s1")?.count).toBe(50_000)
  expect(contextStats(state, "s1")?.percent).toBe(50)
})

test("activity counts distinct tool parts and tracks the running tool", () => {
  const [state, set] = createEngineState()
  reduce(set, toolEvent("p1", "grep", "running"))
  reduce(set, toolEvent("p1", "grep", "completed"))
  reduce(set, toolEvent("p2", "read", "pending"))
  reduce(set, toolEvent("p2", "read", "running"))
  expect(state.activity["child"].tools).toBe(2)
  expect(state.activity["child"].current).toBe("read")
  reduce(set, toolEvent("p2", "read", "completed"))
  expect(state.activity["child"].tools).toBe(2)
  expect(state.activity["child"].current).toBeUndefined()
})

test("session errors terminate busy activity and remain visible", () => {
  const [state, set] = createEngineState()
  set("status", "s1", { type: "busy" })
  set("activity", "s1", { tools: 1, lastPartId: "p1", current: "bash" })
  reduce(
    set,
    {
      type: "session.error",
      properties: { sessionID: "s1", error: { name: "ProviderError", data: { message: "credit balance is too low" } } },
    } as never,
  )
  expect(state.status["s1"].type).toBe("idle")
  expect(state.activity["s1"].current).toBeUndefined()
  expect(state.errors["s1"]).toBe("credit balance is too low")
})

test("current ask events update immediately and retain their workspace directory", () => {
  const [state, set] = createEngineState()
  reduce(
    set,
    {
      type: "permission.asked",
      properties: {
        id: "perm-1",
        sessionID: "s1",
        permission: "bash",
        patterns: ["git status"],
        metadata: { title: "Run command" },
        tool: { messageID: "m1", callID: "c1" },
      },
    } as never,
    "C:/repo",
  )
  reduce(
    set,
    {
      type: "question.asked",
      properties: { id: "q1", sessionID: "s1", questions: [{ question: "Continue?", header: "Continue", options: [] }] },
    } as never,
    "C:/repo",
  )
  expect(state.permissions.s1[0]).toMatchObject({
    id: "perm-1",
    type: "bash",
    pattern: ["git status"],
    title: "Run command",
    metadata: { directory: "C:/repo" },
  })
  expect(state.questions.s1[0].directory).toBe("C:/repo")

  reduce(set, { type: "permission.replied", properties: { sessionID: "s1", requestID: "perm-1" } } as never)
  reduce(set, { type: "question.rejected", properties: { sessionID: "s1", requestID: "q1" } } as never)
  expect(state.permissions.s1).toEqual([])
  expect(state.questions.s1).toEqual([])
})

test("toast and sessionless error events become visible notices", () => {
  const [state, set] = createEngineState()
  reduce(
    set,
    {
      id: "toast-1",
      type: "tui.toast.show",
      properties: { title: "Connected", message: "Provider ready", variant: "success", duration: 2500 },
    } as never,
  )
  reduce(set, { type: "session.error", properties: {} } as never)
  expect(state.notices[0]).toMatchObject({
    id: "toast-1",
    title: "Connected",
    message: "Provider ready",
    variant: "success",
    duration: 2500,
  })
  expect(state.notices[1]).toMatchObject({ title: "Drift error", message: "An error occurred", variant: "error" })
})

test("identical runtime errors collapse into one visible notice", () => {
  const [state, set] = createEngineState()
  const toast = (id: string) =>
    reduce(
      set,
      {
        id,
        type: "tui.toast.show",
        properties: { title: "Drift error", message: "Failed to load plugin", variant: "error" },
      } as never,
    )
  toast("error-1")
  toast("error-2")
  toast("error-3")
  expect(state.notices).toHaveLength(1)
  expect(state.notices[0].id).toBe("error-3")
})

test("a new active status clears stale fallback errors", () => {
  const [state, set] = createEngineState()
  set("errors", "s1", "old failure")
  reduce(set, { type: "session.status", properties: { sessionID: "s1", status: { type: "busy" } } } as never)
  expect(state.errors.s1).toBeUndefined()
})
