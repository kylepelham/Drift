import { expect, test } from "bun:test"
import { createEngineState } from "../src/engine/store"
import { sessionNeedsAttention, sidebarWorkers } from "../src/state/permission-attention"

test("every ask that arrives waits on the user: the engine already answered what auto-accept and always cover", () => {
  const [state, set] = createEngineState()
  expect(sessionNeedsAttention(state, "s1")).toBeFalse()
  set("permissions", "s1", [{ id: "p1", sessionID: "s1", type: "bash", messageID: "m1", title: "bash", metadata: {}, time: { created: 1 } }])
  expect(sessionNeedsAttention(state, "s1")).toBeTrue()
  set("permissions", "s1", [])
  set("questions", "s1", [{ id: "q1", sessionID: "s1", questions: [] }] as never)
  expect(sessionNeedsAttention(state, "s1")).toBeTrue()
})

test("the webview no longer answers asks or keeps an always rule of its own", async () => {
  const composer = await Bun.file("src/ui/composer.tsx").text()
  expect(composer).not.toContain("replyPermission(")
  expect(await Bun.file("src/state/permission-attention.ts").text()).not.toContain("metadata.always")
})

test("the sidebar shows a subagent while it runs, waits on the user, or is open", () => {
  const [state, set] = createEngineState()
  for (const id of ["running", "asking", "done"]) set("sessions", id, { id, parentID: "parent", time: { created: 1, updated: 1 } } as never)
  set("status", "running", { type: "busy" })
  set("status", "done", { type: "idle" })
  set("questions", "asking", [{ id: "q1", sessionID: "asking", questions: [] }] as never)
  expect(sidebarWorkers(state, "parent").map((s) => s.id).sort()).toEqual(["asking", "running"])
  set("status", "running", { type: "idle" })
  set("questions", "asking", [])
  expect(sidebarWorkers(state, "parent")).toEqual([])
  expect(sidebarWorkers(state, "parent", "done").map((s) => s.id)).toEqual(["done"])
})
