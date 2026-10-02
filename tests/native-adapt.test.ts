import { expect, test } from "bun:test"
import { adaptEvent, adaptMessage, adaptPart, adaptPermission, adaptProvider, adaptSession } from "../src/engine/native/adapt"
import type { components } from "../src/engine/native/types"
import { toolElapsedMs } from "../src/ui/tool-duration"

const workspaces = { path: (id: string) => (id === "w1" ? "C:/repo" : undefined), id: () => "w1" }

const session: components["schemas"]["Session"] = {
  id: "ses_1",
  workspaceId: "w1",
  visibility: "sibling",
  title: "Fix bug",
  agent: "build",
  model: { provider: "anthropic", model: "claude" },
  createdAt: 10,
  updatedAt: 20,
}

test("sessions map workspace ids to directories and keep archive time", () => {
  const legacy = adaptSession({ ...session, archivedAt: 30 }, workspaces)
  expect(legacy.directory).toBe("C:/repo")
  expect(legacy.projectID).toBe("w1")
  expect(legacy.time).toEqual({ created: 10, updated: 20, archived: 30 })
  expect((legacy as { model?: { providerID: string; id: string } }).model).toEqual({ providerID: "anthropic", id: "claude" })
})

test("a model's reasoning levels from the catalog become the picker's variants, in order", () => {
  const model = {
    id: "claude-opus-5-5",
    name: "Claude Opus 5.5",
    reasoning: true,
    variants: [
      { name: "low", kind: "effort", level: "low" },
      { name: "max", kind: "effort", level: "max" },
    ],
  }
  const plain = { id: "claude-haiku", name: "Claude Haiku" }
  const provider = { id: "anthropic", name: "Anthropic", models: { [model.id]: model, [plain.id]: plain } } as unknown as Parameters<typeof adaptProvider>[0]
  const models = adaptProvider(provider).models as Record<string, { variants?: Record<string, unknown> }>
  expect(Object.keys(models["claude-opus-5-5"].variants ?? {})).toEqual(["low", "max"])
  expect(models["claude-haiku"].variants).toEqual({})
})

test("subagents nest under their parent while spawned threads stay top level with a link", () => {
  const subagent = adaptSession({ ...session, id: "ses_2", parentId: "ses_1", visibility: "hidden" }, workspaces)
  expect(subagent.parentID).toBe("ses_1")
  const spawned = adaptSession({ ...session, id: "ses_3", parentId: "ses_1" }, workspaces)
  expect(spawned.parentID).toBeUndefined()
  expect((spawned as { spawnedFrom?: string }).spawnedFrom).toBe("ses_1")
})

test("assistant messages carry tokens, cost and errors in the legacy shape", () => {
  const info = adaptMessage(
    {
      id: "msg_1",
      sessionId: "ses_1",
      role: "assistant",
      status: "error",
      model: { provider: "anthropic", model: "claude" },
      usage: { input: 5, output: 7, cacheRead: 1, cacheWrite: 2 },
      cost: 0.5,
      error: "boom",
      createdAt: 1,
      finishedAt: 2,
    },
    "C:/repo",
  )
  if (info.role !== "assistant") throw new Error("expected assistant")
  expect(info.tokens).toEqual({ input: 5, output: 7, reasoning: 0, cache: { read: 1, write: 2 } })
  expect(info.cost).toBe(0.5)
  expect(info.error).toEqual({ name: "UnknownError", data: { message: "boom" } })
  expect(info.time).toEqual({ created: 1, completed: 2 })
})

test("sessions and messages keep the agent they actually ran as", () => {
  expect((adaptSession({ ...session, agent: "plan" }, workspaces) as { agent?: string }).agent).toBe("plan")
  expect((adaptSession({ ...session, variant: "max" }, workspaces) as { variant?: string | null }).variant).toBe("max")
  const base = { sessionId: "ses_1", model: { provider: "anthropic", model: "claude" }, usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, cost: 0, createdAt: 1, agent: "plan" }
  const asked = adaptMessage({ ...base, id: "msg_u", role: "user", status: "done" }, "C:/repo")
  const replied = adaptMessage({ ...base, id: "msg_a", role: "assistant", status: "done" }, "C:/repo")
  expect(asked.role === "user" && asked.agent).toBe("plan")
  expect(replied.role === "assistant" && replied.mode).toBe("plan")
})

test("a reply that stopped at its output limit shows why", () => {
  const base = { id: "msg_3", sessionId: "ses_1", role: "assistant" as const, model: { provider: "anthropic", model: "claude" }, usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, cost: 0, createdAt: 1 }
  type Shown = { finish?: string; error?: { name: string; data: { message: string } } }
  const cut = adaptMessage({ ...base, status: "done", ending: "length", error: "The reply stopped at the output limit (32000 tokens)." }, "C:/repo") as Shown
  expect(cut.finish).toBe("length")
  expect(cut.error).toEqual({ name: "MessageOutputLengthError", data: { message: "The reply stopped at the output limit (32000 tokens)." } })
  const refused = adaptMessage({ ...base, status: "done", ending: "refused", error: "Blocked, in any words." }, "C:/repo") as Shown
  expect(refused.finish).toBe("content-filter")
  expect(refused.error?.data.message).toBe("Blocked, in any words.")
  const paused = adaptMessage({ ...base, status: "paused", error: "Paused after 200 steps, this turn's limit." }, "C:/repo") as { error?: { name: string; data: { message: string } } }
  expect(paused.error).toEqual({ name: "MessageAbortedError", data: { message: "Paused after 200 steps, this turn's limit." } })
  const whole = adaptMessage({ ...base, status: "done" }, "C:/repo") as { finish?: string; error?: unknown }
  expect(whole.finish).toBe("stop")
  expect(whole.error).toBeUndefined()
})

test("a mention keeps the file it names, so undo restores it as a mention and its chip can open it", async () => {
  const { draftFromMessage } = await import("../src/state/composer")
  const row = { id: "prt_m", messageId: "msg_m", sessionId: "ses_1", type: "file" as const, mime: "text/plain", name: "src/db.ts", url: "data:text/plain;base64,eA==", path: "src/db.ts" }
  const mention = adaptPart(row)
  expect(mention).toMatchObject({ type: "file", source: { type: "file", path: "src/db.ts" } })
  const pasted = adaptPart({ ...row, id: "prt_p", name: "notes.txt", path: undefined })
  expect("source" in pasted).toBe(false)
  const entry = { info: { id: "msg_m", sessionID: "ses_1", role: "user", time: { created: 1 } }, parts: [{ id: "t", type: "text", text: "check @src/db.ts" }, mention] } as never
  expect(draftFromMessage(entry).mentions).toEqual(["src/db.ts"])
})

test("one reference is one mention: a path never matches inside a longer one", async () => {
  const { mentionFiles } = await import("../src/ui/composer-mentions")
  const text = "what is in @src/database/database.ts, and @README.md."
  const sent = mentionFiles(text, ["src/database", "src/database/database.ts", "README.md"], "C:/repo")
  expect(sent.map((file) => file.filename)).toEqual(["database.ts", "README.md"])
})

test("a delivered background result is engine text, not the user's words", () => {
  const part = adaptPart({ id: "prt_9", messageId: "msg_9", sessionId: "ses_1", type: "task_result", taskId: "task_1", workerSessionId: "ses_w", description: "Survey", outcome: "replied", text: "three things" })
  expect(part).toMatchObject({ type: "text", synthetic: true, sessionID: "ses_1", text: 'Background task "Survey" replied:\n\nthree things' })
})

test("an async answer becomes the Answered row the transcript already draws", async () => {
  const { clarificationAnswer } = await import("../src/ui/clarification-answer")
  const part = adaptPart({ id: "prt_a", messageId: "msg_a", sessionId: "ses_1", type: "clarification", requestId: "q_1", items: [{ header: "Deploy", question: "Deploy?", answers: ["yes"] }] })
  expect(part).toMatchObject({ type: "text", text: "Deploy?\nAnswer: yes", metadata: { driftClarification: { version: 1, requestID: "q_1" } } })
  const entry = { info: { id: "msg_a", sessionID: "ses_1", role: "user", time: { created: 1 }, agent: "build", model: { providerID: "", modelID: "" } }, parts: [part] } as never
  expect(clarificationAnswer(entry)).toEqual({ items: [{ header: "Deploy", question: "Deploy?", answers: ["yes"] }], text: "Deploy?\nyes", preview: "yes" })
})

test("an async question keeps its flag for the Answer later card", async () => {
  const { adaptQuestion } = await import("../src/engine/native/adapt")
  const request = { id: "q_1", sessionId: "ses_1", messageId: "msg_1", callId: "call_1", createdAt: 1, async: true, questions: [{ question: "Deploy?", header: "Deploy", options: [] }] }
  expect(adaptQuestion(request).async).toBe(true)
  expect(adaptQuestion({ ...request, async: false }).async).toBe(false)
})

test("a compaction becomes the boundary part and summary message the transcript already draws", () => {
  const summary = adaptMessage(
    { id: "msg_2", sessionId: "ses_1", role: "assistant", status: "done", model: { provider: "anthropic", model: "claude" }, usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, cost: 0, createdAt: 1, summary: true },
    "C:/repo",
  )
  expect((summary as { summary?: boolean }).summary).toBeTrue()
  const boundary = adaptPart({ id: "prt_1", messageId: "msg_1", sessionId: "ses_1", type: "compaction", auto: true, tailFrom: "msg_0" })
  expect(boundary).toEqual({ id: "prt_1", sessionID: "ses_1", messageID: "msg_1", type: "compaction", auto: true })
})

test("tool call statuses become legacy tool states", () => {
  const base = { id: "prt_1", messageId: "msg_1", sessionId: "ses_1", type: "tool_call" as const, callId: "t", name: "read", input: { path: "a" } }
  const done = adaptPart({ ...base, status: "done", output: "1: a", title: "a", startedAt: 1, finishedAt: 2 })
  if (done.type !== "tool") throw new Error("expected tool")
  expect(done.tool).toBe("read")
  expect(done.state).toEqual({ status: "completed", input: { path: "a" }, output: "1: a", title: "a", metadata: {}, time: { start: 1, end: 2 } })
  const denied = adaptPart({ ...base, status: "denied", output: "Permission denied by the user." })
  if (denied.type !== "tool") throw new Error("expected tool")
  expect(denied.state.status).toBe("error")
  const pending = adaptPart({ ...base, status: "pending" })
  if (pending.type !== "tool") throw new Error("expected tool")
  expect(pending.state).toEqual({ status: "pending", input: { path: "a" }, raw: "" })
})

test("an undo marker and a removed message reach the reducer in its vocabulary", () => {
  const undone = adaptSession({ ...session, revert: { messageId: "msg_5", kept: ["a.txt"] } }, workspaces)
  expect((undone as { revert?: unknown }).revert).toEqual({ messageID: "msg_5" })
  expect((adaptSession(session, workspaces) as { revert?: unknown }).revert).toBeUndefined()
  expect(adaptEvent({ type: "message.removed", sessionId: "ses_1", messageId: "msg_5" }, workspaces)).toEqual({
    type: "message.removed",
    properties: { sessionID: "ses_1", messageID: "msg_5" },
  })
})

test("a retry wait becomes the retry status the notice draws", () => {
  expect(adaptEvent({ type: "session.retry", sessionId: "ses_1", attempt: 2, message: "overloaded (529): busy", nextAt: 5000 }, workspaces)).toEqual({
    type: "session.status",
    properties: { sessionID: "ses_1", status: { type: "retry", attempt: 2, message: "overloaded (529): busy", next: 5000 } },
  })
})

test("events translate to the legacy reducer's vocabulary", () => {
  expect(adaptEvent({ type: "session.status", sessionId: "ses_1", status: "running" }, workspaces)).toEqual({
    type: "session.status",
    properties: { sessionID: "ses_1", status: { type: "busy" } },
  })
  const delta = adaptEvent({ type: "part.delta", sessionId: "s", messageId: "m", partId: "p", delta: "hi" }, workspaces)
  expect(delta).toEqual({ type: "message.part.delta", properties: { sessionID: "s", messageID: "m", partID: "p", field: "text", delta: "hi" } })
  const request: components["schemas"]["PermissionRequest"] = {
    id: "perm_1",
    sessionId: "ses_1",
    messageId: "msg_1",
    callId: "t",
    tool: "bash",
    kind: "bash",
    pattern: "cargo test",
    title: "Run tests",
    createdAt: 5,
  }
  const asked = adaptEvent({ type: "permission.asked", request }, workspaces)
  expect(asked?.type).toBe("permission.updated")
  expect(adaptPermission(request, "C:/repo")).toMatchObject({ id: "perm_1", type: "bash", pattern: ["cargo test"], callID: "t", metadata: { directory: "C:/repo", tool: "bash" } })
  expect(adaptEvent({ type: "workspace.created", workspace: { id: "w", path: "p", name: "n", icon: "", lastUsed: 0 } }, workspaces)).toBeUndefined()
})

test("a call that never started has no duration", () => {
  const row = { id: "prt_1", messageId: "msg_1", sessionId: "ses_1", type: "tool_call" as const, callId: "c1", name: "bash", input: { command: "sleep 10" }, status: "denied" as const, output: "denied", finishedAt: 1_700_000_000_000 }
  const part = adaptPart(row as never)
  expect(part.type).toBe("tool")
  const state = (part as { state: { time?: { start?: number; end?: number } } }).state
  expect(state.time?.start).toBeUndefined()
  expect(state.time?.end).toBe(1_700_000_000_000)
  expect(toolElapsedMs(state as never, Date.now())).toBeUndefined()
})
