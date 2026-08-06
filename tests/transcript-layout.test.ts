import { expect, test } from "bun:test"
import type { MessageEntry } from "../src/engine/store"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

const tool = (id: string, messageID: string, name = "read") => ({
  id,
  messageID,
  sessionID: "s1",
  type: "tool",
  tool: name,
  state: { status: "completed", input: {}, output: "", title: "", metadata: {}, time: { start: 1, end: 2 } },
})

const text = (id: string, messageID: string) => ({
  id,
  messageID,
  sessionID: "s1",
  type: "text",
  text: id,
  time: { start: 1, end: 2 },
})

const assistant = (id: string, parts: unknown[], extra: Record<string, unknown> = {}) => ({
  info: { id, sessionID: "s1", role: "assistant", time: { created: 1 }, ...extra },
  parts,
}) as MessageEntry

test("assistant grouping and pitch are invariant to provider message chunking", async () => {
  const { groupAssistantEntries } = await import("../src/ui/message")
  const { timelinePitch } = await import("../src/ui/chat")
  const one = [assistant("a1", [tool("r1", "a1"), tool("r2", "a1"), tool("r3", "a1"), text("answer", "a1")])]
  const split = [
    assistant("a1", [tool("r1", "a1"), tool("r2", "a1")]),
    assistant("a2", [tool("r3", "a2"), text("answer", "a2")]),
  ]
  const structure = (entries: MessageEntry[]) => {
    const grouped = groupAssistantEntries(entries)
    const visible = entries.flatMap((entry) => (grouped.get(entry.info.id) ?? []).map((group) => ({ entry, group })))
    return visible.map(({ entry, group }, index) => ({
      type: "explored" in group ? "context" : group.part.type,
      parts: "explored" in group ? group.explored.map((part) => part.id) : [group.part.id],
      pitch: timelinePitch(entry, visible[index + 1]?.entry),
    }))
  }

  expect(structure(one)).toEqual([
    { type: "context", parts: ["r1", "r2", "r3"], pitch: "part" },
    { type: "text", parts: ["answer"], pitch: "none" },
  ])
  expect(structure(split)).toEqual(structure(one))
})

test("context grouping stops at meaningful transcript boundaries", async () => {
  const { groupAssistantEntries } = await import("../src/ui/message")
  const first = assistant("a1", [tool("r1", "a1")])
  const user = {
    info: { id: "u1", sessionID: "s1", role: "user", time: { created: 2 } },
    parts: [text("question", "u1")],
  } as MessageEntry
  const second = assistant("a2", [tool("r2", "a2")])
  const grouped = groupAssistantEntries([first, user, second])

  expect(grouped.get("a1")?.map((group) => "explored" in group ? group.explored.length : 0)).toEqual([1])
  expect(grouped.get("a2")?.map((group) => "explored" in group ? group.explored.length : 0)).toEqual([1])
})

test("timeline pitch keeps turn, compaction, and error breaks without trailing space", async () => {
  const { timelinePitch } = await import("../src/ui/chat")
  const regular = assistant("a1", [text("one", "a1")])
  const continuation = assistant("a2", [text("two", "a2")])
  const summary = assistant("a3", [text("summary", "a3")], { summary: true })
  const failed = assistant("a4", [], { error: { name: "ProviderError" } })
  const user = {
    info: { id: "u1", sessionID: "s1", role: "user", time: { created: 2 } },
    parts: [text("question", "u1")],
  } as MessageEntry

  expect(timelinePitch(regular, continuation)).toBe("part")
  expect(timelinePitch(regular, user)).toBe("turn")
  expect(timelinePitch(regular, summary)).toBe("turn")
  expect(timelinePitch(regular, failed)).toBe("turn")
  expect(timelinePitch(regular)).toBe("none")
})

test("tokens per second uses generation time, not tool and subagent wall time", async () => {
  const { generationMs, tokensPerSecond } = await import("../src/ui/message")
  const entry = {
    info: {
      id: "a1",
      sessionID: "s1",
      role: "assistant",
      time: { created: 0, completed: 600_000 },
      tokens: { input: 10, output: 500, reasoning: 100, cache: { read: 0, write: 0 } },
      cost: 0,
      modelID: "m",
    },
    parts: [
      { id: "p1", messageID: "a1", sessionID: "s1", type: "reasoning", text: "r", time: { start: 0, end: 2_000 } },
      { id: "p2", messageID: "a1", sessionID: "s1", type: "tool", tool: "task", state: { status: "completed", input: {}, output: "", title: "", metadata: {}, time: { start: 2_000, end: 590_000 } } },
      { id: "p3", messageID: "a1", sessionID: "s1", type: "text", text: "answer", time: { start: 590_000, end: 600_000 } },
    ],
  } as never
  expect(generationMs(entry)).toBe(12_000)
  // 600 tokens over 12s of generation, not 600s of wall time.
  expect(tokensPerSecond(entry)).toBe("50.0")

  const openEnded = {
    info: {
      id: "a2",
      sessionID: "s1",
      role: "assistant",
      time: { created: 0, completed: 20_000 },
      tokens: { input: 1, output: 100, reasoning: 0, cache: { read: 0, write: 0 } },
    },
    parts: [{ id: "p1", messageID: "a2", sessionID: "s1", type: "text", text: "t", time: { start: 10_000 } }],
  } as never
  // An unterminated part falls back to the message completion time.
  expect(generationMs(openEnded)).toBe(10_000)
  expect(tokensPerSecond(openEnded)).toBe("10.0")
})
