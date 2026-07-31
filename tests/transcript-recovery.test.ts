import { expect, test } from "bun:test"
import type { Message, Part, Session } from "@opencode-ai/sdk/client"
import { createActions } from "../src/engine/actions"
import { createEngineState, type MessageEntry } from "../src/engine/store"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

function entry(id: string): MessageEntry {
  return {
    info: { id, sessionID: "session", role: "user", time: { created: 1 } } as Message,
    parts: [{ id: `part-${id}`, sessionID: "session", messageID: id, type: "text", text: id } as Part],
  }
}

function harness(messages: () => Promise<unknown>) {
  const [state, set] = createEngineState()
  const actions = createActions(
    () => ({ session: { messages } }) as never,
    state,
    set,
    () => ({ url: "http://engine.test" }),
  )
  return { state, set, actions }
}

test("failed transcript loads preserve state and remain retryable", async () => {
  let requests = 0
  const fresh = entry("fresh")
  const { state, set, actions } = harness(async () => {
    requests += 1
    if (requests === 1) throw new Error("network down")
    return { data: [fresh], response: { headers: new Headers({ "x-next-cursor": "older" }) } }
  })
  const existing = entry("existing")
  set("transcripts", "session", [existing])
  set("cursors", "session", "keep")

  expect(await actions.openSession("session")).toBeFalse()
  expect(state.loaded.session).toBeUndefined()
  expect(state.transcripts.session).toEqual([existing])
  expect(state.cursors.session).toBe("keep")
  expect(state.notices.at(-1)?.title).toBe("Transcript load failed")

  expect(await actions.openSession("session")).toBeTrue()
  expect(requests).toBe(2)
  expect(state.loaded.session).toBeTrue()
  expect(state.transcripts.session).toEqual([fresh])
  expect(state.cursors.session).toBe("older")
})

test("SDK transcript errors do not replace existing data", async () => {
  const existing = entry("existing")
  const { state, set, actions } = harness(async () => ({ error: { message: "engine rejected the load" } }))
  set("transcripts", "session", [existing])

  expect(await actions.openSession("session")).toBeFalse()
  expect(state.loaded.session).toBeUndefined()
  expect(state.transcripts.session).toEqual([existing])
  expect(state.notices.at(-1)?.message).toBe("engine rejected the load")
})

test("concurrent transcript opens share one request", async () => {
  let requests = 0
  let release!: () => void
  const pending = new Promise<void>((resolve) => (release = resolve))
  const { state, actions } = harness(async () => {
    requests += 1
    await pending
    return { data: [entry("loaded")] }
  })

  const first = actions.openSession("session")
  const second = actions.openSession("session")
  expect(first).toBe(second)
  expect(requests).toBe(1)
  release()
  expect(await Promise.all([first, second])).toEqual([true, true])
  expect(state.loaded.session).toBeTrue()
})

test("a successful empty transcript is authoritative", async () => {
  const { state, set, actions } = harness(async () => ({ data: [] }))
  set("transcripts", "session", [entry("existing")])
  set("cursors", "session", "keep")

  expect(await actions.openSession("session")).toBeTrue()
  expect(state.loaded.session).toBeTrue()
  expect(state.transcripts.session).toEqual([])
  expect(state.cursors.session).toBeNull()
})

test("revert actions remain successful when their transcript refresh fails", async () => {
  const session = {
    id: "session",
    slug: "session",
    projectID: "project",
    directory: "C:/work",
    title: "Session",
    version: "test",
    time: { created: 1, updated: 1 },
  } as Session
  const [state, set] = createEngineState()
  const actions = createActions(
    () => ({
      session: {
        revert: async () => ({ data: session }),
        unrevert: async () => ({ data: session }),
        messages: async () => {
          throw new Error("refresh failed")
        },
      },
    }) as never,
    state,
    set,
    () => ({ url: "http://engine.test" }),
  )

  expect(await actions.revert("session", "message")).toBeTrue()
  expect(state.notices.at(-1)?.message).toBe("refresh failed")
  expect(await actions.unrevert("session")).toBeTrue()
  expect(state.notices.at(-1)?.message).toBe("refresh failed")
})
