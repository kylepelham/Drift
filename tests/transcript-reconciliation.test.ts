import { expect, test } from "bun:test"
import { createActions } from "../src/engine/actions"
import { reduce, withDelta } from "../src/engine/events"
import { adaptEvent, adaptMessage, adaptPart, adaptSession } from "../src/engine/native/adapt"
import type { Client, MessageWithParts } from "../src/engine/native/client"
import { captureRevisions, createEngineState, mergeTranscriptSnapshot, messageRevisionKey, pruneSessionRevisions, removedPartKey } from "../src/engine/store"

const workspaces = { path: () => "C:/repo", id: () => "w" }

function message(text: string): MessageWithParts {
  return {
    id: "m", sessionId: "s", role: "assistant", status: "streaming", createdAt: 1, cost: 0,
    usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    parts: [{ id: "p", sessionId: "s", messageId: "m", type: "text", text }],
  }
}

function entry(text: string) {
  const { parts, ...info } = message(text)
  return { info: adaptMessage(info, "C:/repo"), parts: parts.map(adaptPart) }
}

test("a delta gap leaves the cached prefix and revision unchanged and requests reconciliation", () => {
  const [state, set] = createEngineState()
  set("transcripts", "s", [entry("hel")])
  const requested: string[] = []
  const event = adaptEvent({ type: "part.delta", sessionId: "s", messageId: "m", partId: "p", offset: 6, delta: "world" }, workspaces)!
  reduce(set, event, undefined, (id) => requested.push(id))
  expect(state.transcripts.s![0]!.parts[0]).toMatchObject({ text: "hel" })
  expect(state.revisions[messageRevisionKey("s", "m")]).toBeUndefined()
  expect(requested).toEqual(["s"])
  expect(withDelta("hel", "world", 6)).toBe("hel")
})

test("a snapshot repairs a shorter prefix even when a live message revision advanced", () => {
  const [state, set] = createEngineState()
  set("loaded", "s", true)
  set("transcripts", "s", [entry("hel")])
  const captured = captureRevisions(state)
  const update = adaptEvent({ type: "message.updated", message: { ...message(""), status: "done" } }, workspaces)!
  reduce(set, update)
  const merged = mergeTranscriptSnapshot(state.transcripts.s, [entry("hello world")], "s", captured, state.revisions)
  expect(merged[0]!.parts[0]).toMatchObject({ text: "hello world" })
  expect(merged[0]!.info).toMatchObject({ finish: "stop" })
  const ahead = mergeTranscriptSnapshot([entry("hello world!")], [entry("hello")], "s", {}, { [messageRevisionKey("s", "m")]: 1 })
  expect(ahead[0]!.parts[0]).toMatchObject({ text: "hello world!" })
})

test("a gap during an HTTP reload fetches a newer snapshot after that reload finishes", async () => {
  let complete!: (messages: MessageWithParts[]) => void
  let calls = 0
  const client = {
    messages: () => ++calls === 1 ? new Promise<MessageWithParts[]>((resolve) => (complete = resolve)) : Promise.resolve([message("hello world")]),
    todos: () => Promise.resolve([]), tasks: () => Promise.resolve([]),
  } as unknown as Client
  const [state, set] = createEngineState()
  set("sessions", "s", adaptSession({ id: "s", workspaceId: "w", visibility: "sibling", title: "", agent: "build", createdAt: 1, updatedAt: 1 }, workspaces))
  set("transcripts", "s", [entry("hel")])
  const actions = createActions(() => client, state, set, () => workspaces)
  const loading = actions.openSession("s")
  const reconciling = actions.reconcileSession("s")
  expect(actions.reconcileSession("s")).toBe(reconciling)
  expect(calls).toBe(1)
  complete([message("hello ")])
  await loading
  await reconciling
  expect(calls).toBe(2)
  expect(state.transcripts.s![0]!.parts[0]).toMatchObject({ text: "hello world" })
})

test("a snapshot restores an entire missed part after a live message revision advances", () => {
  const [state, set] = createEngineState()
  const cached = entry("")
  cached.parts = []
  set("loaded", "s", true)
  set("transcripts", "s", [cached])
  const captured = captureRevisions(state)
  reduce(set, adaptEvent({ type: "message.updated", message: { ...message(""), status: "done" } }, workspaces)!)
  let reconciled = 0
  reduce(set, adaptEvent({ type: "part.delta", sessionId: "s", messageId: "m", partId: "p", delta: "world", offset: 6 }, workspaces)!, undefined, () => reconciled++)
  expect(reconciled).toBe(1)
  const merged = mergeTranscriptSnapshot(state.transcripts.s, [entry("hello world")], "s", captured, state.revisions)
  expect(merged[0]!.parts).toHaveLength(1)
  expect(merged[0]!.parts[0]).toMatchObject({ id: "p", text: "hello world" })
  expect(merged[0]!.info).toMatchObject({ finish: "stop" })
})

test("explicit part removal survives both racing and later stale snapshots", () => {
  const [state, set] = createEngineState()
  set("loaded", "s", true)
  set("transcripts", "s", [entry("hello world")])
  const captured = captureRevisions(state)
  reduce(set, { type: "message.part.removed", properties: { sessionID: "s", messageID: "m", partID: "p" } })
  expect(state.revisions[removedPartKey("s", "m", "p")]).toBe(1)
  let reconciled = 0
  reduce(set, adaptEvent({ type: "part.delta", sessionId: "s", messageId: "m", partId: "p", delta: "late", offset: 20 }, workspaces)!, undefined, () => reconciled++)
  expect(reconciled).toBe(0)
  let merged = mergeTranscriptSnapshot(state.transcripts.s, [entry("hello world")], "s", captured, state.revisions)
  expect(merged[0]!.parts).toEqual([])
  merged = mergeTranscriptSnapshot(merged, [entry("hello world")], "s", captureRevisions(state), state.revisions)
  expect(merged[0]!.parts).toEqual([])
  reduce(set, adaptEvent({ type: "part.updated", part: message("restored").parts[0]! }, workspaces)!)
  expect(state.revisions[removedPartKey("s", "m", "p")]).toBeUndefined()
  expect(state.transcripts.s![0]!.parts[0]).toMatchObject({ text: "restored" })
  set("revisions", { [removedPartKey("s", "m", "p")]: 1 })
  const draft = { ...state, revisions: { ...state.revisions } }
  pruneSessionRevisions(draft, "s")
  expect(draft.revisions).toEqual({})
})

test("a removal remembered without a cached part still prevents snapshot resurrection", () => {
  const [state, set] = createEngineState()
  reduce(set, { type: "message.part.removed", properties: { sessionID: "s", messageID: "m", partID: "p" } })
  const merged = mergeTranscriptSnapshot(undefined, [entry("hello world")], "s", captureRevisions(state), state.revisions)
  expect(merged[0]!.parts).toEqual([])
})
