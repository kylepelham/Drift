import type { Message, Part, Session } from "@opencode-ai/sdk/client"
import type { SetStoreFunction } from "solid-js/store"
import type { EngineState, MessageEntry } from "./store"

const png =
  "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="

const code = "```ts\nexport function bench(value: number) {\n  return value * 2\n}\n```"

function id(prefix: string, index: number, suffix = "") {
  return `${prefix}_bench${String(index).padStart(8, "0")}${suffix}`
}

function userEntry(index: number, sessionID: string): MessageEntry {
  const info = {
    id: id("msg", index),
    sessionID,
    role: "user",
    time: { created: Date.now() - (10000 - index) * 60000 },
    model: { providerID: "anthropic", modelID: "claude-sonnet-4-6" },
    agent: "build",
  } as unknown as Message
  const parts = [
    { id: id("prt", index, "a"), messageID: info.id, sessionID, type: "text", text: `Bench prompt ${index}: do the thing.` },
  ] as unknown as Part[]
  if (index % 6 === 0)
    parts.unshift({
      id: id("prt", index, "img"),
      messageID: info.id,
      sessionID,
      type: "file",
      mime: "image/png",
      filename: `shot-${index}.png`,
      url: png,
    } as unknown as Part)
  return { info, parts }
}

function assistantEntry(index: number, sessionID: string): MessageEntry {
  const info = {
    id: id("msg", index),
    sessionID,
    role: "assistant",
    time: { created: Date.now() - (10000 - index) * 60000, completed: Date.now() - (10000 - index) * 60000 + 4000 },
    modelID: "claude-sonnet-4-6",
    providerID: "anthropic",
    tokens: { input: 1200, output: 300, reasoning: 0, cache: { read: 0, write: 0 } },
    cost: 0.01,
  } as unknown as Message
  const parts: Part[] = []
  if (index % 4 === 0)
    parts.push({
      id: id("prt", index, "t"),
      messageID: info.id,
      sessionID,
      type: "tool",
      callID: id("call", index),
      tool: "bash",
      state: {
        status: "completed",
        input: { command: `echo bench ${index}` },
        output: `bench ${index}\nline two\nline three`,
        title: "bash",
        metadata: {},
        time: { start: 0, end: 1 },
      },
    } as unknown as Part)
  parts.push({
    id: id("prt", index, "b"),
    messageID: info.id,
    sessionID,
    type: "text",
    text:
      index % 5 === 0
        ? `Answer ${index} with some **markdown**, a list:\n\n- alpha\n- beta\n\n${code}`
        : `Answer ${index}: a couple of sentences of plain response text to vary row heights a little.`,
  } as unknown as Part)
  return { info, parts }
}

export function seedBench(set: SetStoreFunction<EngineState>, directory: string) {
  const count = Number(new URLSearchParams(location.search).get("bench")) || 0
  if (!count || !directory) return
  const sessionID = "ses_bench0000000000000000000000"
  const entries: MessageEntry[] = []
  for (let index = 0; index < count; index++)
    entries.push(index % 2 === 0 ? userEntry(index, sessionID) : assistantEntry(index, sessionID))
  const middle = entries[Math.floor(count / 2)]
  if (middle?.info.role === "assistant")
    middle.parts.unshift({
      id: id("prt", Math.floor(count / 2), "c"),
      messageID: middle.info.id,
      sessionID,
      type: "compaction",
      auto: true,
    } as unknown as Part)
  const tail = entries[count - 1]
  if (tail && "tokens" in tail.info)
    (tail.info as { tokens: unknown }).tokens = {
      input: 2,
      output: 310,
      reasoning: 0,
      cache: { read: 150000, write: 6000 },
      total: 156312,
    }
  const session = {
    id: sessionID,
    title: `Bench ${count}`,
    directory,
    time: { created: Date.now(), updated: Date.now() },
    version: "bench",
  } as unknown as Session
  set("sessions", sessionID, session)
  set("transcripts", sessionID, entries)
  set("loaded", sessionID, true)
  set("cursors", sessionID, null)
}
