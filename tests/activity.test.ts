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
