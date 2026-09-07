import { expect, test } from "bun:test"
import type { Message, Part, Session } from "@opencode-ai/sdk/client"
import { createActions } from "../src/engine/actions"
import { reduce as reduceEvent } from "../src/engine/events"
import { createEngineState, messageRevisionKey, type MessageEntry } from "../src/engine/store"
import { createComposerSubmit } from "../src/ui/composer-submit"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => (resolve = done))
  return { promise, resolve }
}

const session = {
  id: "session",
  slug: "session",
  projectID: "project",
  directory: "C:/work",
  title: "New session",
  version: "test",
  time: { created: 1, updated: 1 },
} as Session

function entry(id: string, role: "user" | "assistant" = "user", text = id): MessageEntry {
  return {
    info: {
      id,
      sessionID: session.id,
      role,
      time: { created: role === "user" ? 1 : 2 },
      ...(role === "assistant" ? { providerID: "test", modelID: "test", parentID: "user" } : {}),
    } as Message,
    parts: [{ id: `part-${id}`, sessionID: session.id, messageID: id, type: "text", text } as Part],
  }
}

function harness(messages: () => Promise<unknown>, create = async (): Promise<unknown> => ({ data: session })) {
  const [state, set] = createEngineState()
  let sends = 0
  const emit = (type: string, properties: unknown) => reduceEvent(set, { type, properties } as never)
  const publish = (message: MessageEntry) => {
    emit("message.updated", { info: message.info })
    for (const part of message.parts) emit("message.part.updated", { part })
  }
  const delta = (text: string) => emit("message.part.delta", {
    sessionID: session.id, messageID: "assistant", partID: "part-assistant", field: "text", delta: text,
  })
  const stream = () => {
    emit("session.status", { sessionID: session.id, status: { type: "busy" } })
    publish(entry("user"))
    publish(entry("assistant", "assistant", "Hello"))
    delta(" world")
    emit("message.updated", { info: { ...entry("assistant", "assistant").info, time: { created: 2, completed: 3 } } })
    emit("session.idle", { sessionID: session.id })
  }
  const actions = createActions(() => ({ session: {
    create,
    messages,
    promptAsync: async () => {
      sends++
      stream()
      return { data: {} }
    },
  } }) as never, state, set, () => undefined)
  return { state, set, actions, emit, publish, delta, stream, sends: () => sends }
}

test("new-session composer first send keeps the assistant without waiting for a transcript GET", async () => {
  const snapshot = deferred<unknown>()
  let gets = 0
  const { state, actions, sends } = harness(() => { gets++; return snapshot.promise })
  let selected: string | null = null
  let open: Promise<boolean> | undefined
  let readyAtSelection = false
  const submit = createComposerSubmit({
    scope: () => selected ?? "new-workspace",
    session: () => selected,
    workspace: () => ({ id: "workspace", name: "Work", path: session.directory }),
    online: () => true,
    draft: () => ({ text: "user", mentions: [], staged: [] }),
    prepare: () => ({}),
    transform: async ({ text }) => text,
    newSession: actions.newSession,
    sessionScope: (id) => id,
    migrateDraft: () => undefined,
    sessionCreated: () => undefined,
    selectSession: (id) => {
      readyAtSelection = state.loaded[id] === true && state.transcripts[id]?.length === 0
      selected = id
      open = actions.openSession(id)
    },
    send: (id, text) => actions.send(id, text, { model: null, agent: "build" }),
    admitted: () => undefined,
  })

  expect(await submit()).toBe("submitted")
  // On the broken path this delayed user-only GET lands after every assistant event was lost.
  snapshot.resolve({ data: [entry("user")] })
  expect(await open).toBeTrue()
  expect(state.transcripts.session?.map((message) => message.info.id)).toEqual(["user", "assistant"])
  expect(state.transcripts.session?.[1]?.parts[0]).toMatchObject({ text: "Hello world" })
  expect(state.transcripts.session?.[1]?.info.time).toMatchObject({ completed: 3 })
  expect(state.status.session?.type).toBe("idle")
  expect(readyAtSelection).toBeTrue()
  expect(gets).toBe(0)
  expect(sends()).toBe(1)
  expect(state.cursors.session).toBeNull()
})

test("local creation establishes known-empty readiness before returning without fetching history", async () => {
  const created = deferred<unknown>()
  let gets = 0
  const { state, actions } = harness(async () => { gets++; return { data: [] } }, () => created.promise)
  const creating = actions.newSession()
  expect(state.transcripts.session).toBeUndefined()
  expect(state.loaded.session).toBeUndefined()
  created.resolve({ data: session })
  expect((await creating)?.id).toBe(session.id)
  expect(state.transcripts.session).toEqual([])
  expect(state.loaded.session).toBeTrue()
  expect(state.cursors.session).toBeNull()
  expect(await actions.openSession(session.id)).toBeTrue()
  expect(gets).toBe(0)
})

for (const failure of ["sdk", "transport", "missing"] as const) {
  test(`failed ${failure} creation does not publish a session or prime transcript readiness`, async () => {
    const { state, actions, sends } = harness(async () => ({ data: [] }), async () => {
      if (failure === "transport") throw new Error("network down")
      return failure === "sdk" ? { error: { message: "create rejected" } } : {}
    })
    if (failure === "transport") await expect(actions.newSession()).rejects.toThrow("network down")
    else expect(await actions.newSession()).toBeUndefined()
    expect(state.sessions).toEqual({})
    expect(state.transcripts).toEqual({})
    expect(state.loaded).toEqual({})
    expect(state.cursors).toEqual({})
    expect(sends()).toBe(0)
  })
}

test("local creation preserves transcript, cursor and live state established before its response", async () => {
  const created = deferred<unknown>()
  const { state, set, actions, emit, publish } = harness(async () => ({ data: [] }), () => created.promise)
  const creating = actions.newSession()
  emit("session.created", { info: session })
  set("transcripts", session.id, [])
  set("loaded", session.id, true)
  publish(entry("assistant", "assistant", "already here"))
  set("cursors", session.id, "keep")
  emit("session.status", { sessionID: session.id, status: { type: "busy" } })
  const transcript = state.transcripts.session
  const revision = state.revisions[messageRevisionKey(session.id, "assistant")]
  created.resolve({ data: session })
  expect((await creating)?.id).toBe(session.id)
  expect(state.transcripts.session).toBe(transcript)
  expect(state.transcripts.session?.[0]?.parts[0]).toMatchObject({ text: "already here" })
  expect(state.cursors.session).toBe("keep")
  expect(state.status.session?.type).toBe("busy")
  expect(state.revisions[messageRevisionKey(session.id, "assistant")]).toBe(revision)
})

test("completion-only events during an ordinary initial GET cannot replace full snapshot parts", async () => {
  const snapshot = deferred<unknown>()
  const { state, actions, emit } = harness(() => snapshot.promise)
  emit("session.created", { info: session })
  const open = actions.openSession(session.id)
  const assistant = entry("assistant", "assistant", "Full persisted answer")
  assistant.info.time = { created: 2, completed: 3 } as Message["time"]
  emit("message.updated", { info: assistant.info })
  expect(state.loaded.session).toBeUndefined()
  snapshot.resolve({ data: [entry("user"), assistant] })
  expect(await open).toBeTrue()
  expect(state.transcripts.session?.[1]?.parts[0]).toMatchObject({ text: "Full persisted answer" })
  expect(state.loaded.session).toBeTrue()
})
