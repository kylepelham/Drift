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
  expect(thinkingAfterMessage([first, response, steer, steeredResponse], "idle")).toBeNull()
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
