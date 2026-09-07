import { expect, test } from "bun:test"
import type { Message, Part, Session } from "@opencode-ai/sdk/client"
import { createActions } from "../src/engine/actions"
import { applySessionSnapshot, applyStatusSnapshot, reduce } from "../src/engine/events"
import { captureRevisions, createEngineState, mergeTranscriptSnapshot, putSession, type MessageEntry } from "../src/engine/store"

function session(id: string, directory = "C:/work"): Session {
  return {
    id,
    slug: id,
    projectID: "project",
    directory,
    title: id,
    version: "test",
    time: { created: 1, updated: 1 },
  } as Session
}

function entry(id: string, text = id): MessageEntry {
  return {
    info: { id, sessionID: "session", role: "user", time: { created: 1 } } as Message,
    parts: [{ id: `part-${id}`, sessionID: "session", messageID: id, type: "text", text } as Part],
  }
}

function listHarness(list: () => Promise<unknown>) {
  const [state, set] = createEngineState()
  const actions = createActions(() => ({ session: { list } }) as never, state, set, () => ({ url: "http://engine.test" }))
  return { state, set, actions }
}

function messagesHarness(messages: () => Promise<unknown>) {
  const [state, set] = createEngineState()
  const actions = createActions(
    () => ({ session: { messages } }) as never,
    state,
    set,
    () => ({ url: "http://engine.test" }),
  )
  return { state, set, actions }
}

test("a complete session snapshot removes sessions deleted during an event gap", async () => {
  const kept = session("kept")
  const { state, set, actions } = listHarness(async () => ({ data: [kept] }))
  putSession(set, kept)
  putSession(set, session("ghost"))
  putSession(set, session("elsewhere", "C:/other"))
  set("status", "ghost", { type: "busy" })
  set("transcripts", "ghost", [entry("m1")])
  set("loaded", "ghost", true)

  await actions.loadSessions("C:/work")
  expect(state.sessions.kept).toBeDefined()
  expect(state.sessions.ghost).toBeUndefined()
  expect(state.status.ghost).toBeUndefined()
  expect(state.transcripts.ghost).toBeUndefined()
  expect(state.loaded.ghost).toBeUndefined()
  expect(state.sessions.elsewhere).toBeDefined()
})

test("a complete session snapshot never purges engine-archived sessions it cannot list", async () => {
  const kept = session("kept")
  const archived = { ...session("archived"), time: { created: 1, updated: 1, archived: 2 } } as Session
  const { state, set, actions } = listHarness(async () => ({ data: [kept] }))
  putSession(set, kept)
  putSession(set, archived)
  set("transcripts", "archived", [entry("m1")])
  set("loaded", "archived", true)

  await actions.loadSessions("C:/work")
  expect(state.sessions.archived).toBeDefined()
  expect(state.transcripts.archived).toBeDefined()
  expect(state.loaded.archived).toBeTrue()
})

test("an errored session snapshot does not remove sessions", async () => {
  const { state, set, actions } = listHarness(async () => ({ error: { message: "engine rejected the list" } }))
  putSession(set, session("kept"))

  await actions.loadSessions("C:/work")
  expect(state.sessions.kept).toBeDefined()
})

test("a truncated session snapshot upserts without removing sessions", async () => {
  const bulk = Array.from({ length: 100 }, (_, index) => session(`bulk-${index}`))
  const { state, set, actions } = listHarness(async () => ({ data: bulk }))
  putSession(set, session("survivor"))

  await actions.loadSessions("C:/work")
  expect(state.sessions.survivor).toBeDefined()
  expect(state.sessions["bulk-0"]).toBeDefined()
})

test("a session deleted while the snapshot was in flight is not resurrected", async () => {
  let release!: () => void
  const pending = new Promise<void>((resolve) => (release = resolve))
  const doomed = session("doomed")
  const { state, set, actions } = listHarness(async () => {
    await pending
    return { data: [doomed] }
  })
  putSession(set, doomed)

  const load = actions.loadSessions("C:/work")
  reduce(set, { type: "session.deleted", properties: { info: doomed } } as never)
  release()
  await load
  expect(state.sessions.doomed).toBeUndefined()
})

test("a session created while the snapshot was in flight survives reconciliation", async () => {
  let release!: () => void
  const pending = new Promise<void>((resolve) => (release = resolve))
  const kept = session("kept")
  const { state, set, actions } = listHarness(async () => {
    await pending
    return { data: [kept] }
  })
  putSession(set, kept)

  const load = actions.loadSessions("C:/work")
  reduce(set, { type: "session.created", properties: { info: session("fresh") } } as never)
  release()
  await load
  expect(state.sessions.fresh).toBeDefined()
  expect(state.sessions.kept).toBeDefined()
})

test("a session updated while the snapshot was in flight keeps the newer event data", () => {
  const [state, set] = createEngineState()
  putSession(set, session("live"))
  const captured = captureRevisions(state)
  reduce(set, { type: "session.updated", properties: { info: { ...session("live"), title: "renamed" } } } as never)

  applySessionSnapshot(set, { sessions: [session("live")], captured, scope: { directory: "C:/work" } })
  expect(state.sessions.live?.title).toBe("renamed")
})

test("a newer status event survives an older delayed status snapshot", () => {
  const [state, set] = createEngineState()
  putSession(set, session("racing"))
  putSession(set, session("settled"))
  const captured = captureRevisions(state)
  reduce(set, { type: "session.status", properties: { sessionID: "racing", status: { type: "busy" } } } as never)

  applyStatusSnapshot(set, {
    sessions: [session("racing"), session("settled"), session("missing")],
    statuses: { settled: { type: "retry", attempt: 1, message: "retrying", next: 2 } },
    captured,
  })
  expect(state.status.racing?.type).toBe("busy")
  expect(state.status.settled?.type).toBe("retry")
  expect(state.status.missing).toBeUndefined()
})

test("hydrate only reconciles successful status data and preserves newer busy events", async () => {
  const ts = await import("typescript")
  const source = await Bun.file(new URL("../src/engine/index.tsx", import.meta.url)).text()
  const parsed = ts.createSourceFile("index.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX)
  const statements: import("typescript").Node[] = []
  function visit(node: import("typescript").Node) {
    if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === "applyStatusSnapshot") {
      let owner: import("typescript").Node = node
      while (owner.parent && !ts.isFunctionDeclaration(owner)) owner = owner.parent
      if (ts.isFunctionDeclaration(owner) && owner.name?.text === "hydrate") {
        // Keep the enclosing guard, not a copied condition or an unguarded helper call.
        let statement: import("typescript").Node = node
        while (!ts.isBlock(statement.parent)) statement = statement.parent
        statements.push(statement)
      }
    }
    ts.forEachChild(node, visit)
  }
  visit(parsed)
  expect(statements).toHaveLength(1)
  const reconcile = new Function(
    "statuses", "set", "list", "captured", "applyStatusSnapshot",
    ts.transpileModule(statements[0]!.getText(parsed), {
      compilerOptions: { target: ts.ScriptTarget.ESNext },
    }).outputText,
  )

  for (const scenario of [
    { name: "error without data", response: { error: { message: "status unavailable" } }, applied: false },
    { name: "error with empty data", response: { error: { message: "status unavailable" }, data: {} }, applied: false },
    { name: "null error is not success", response: { error: null, data: {} }, applied: false },
    { name: "missing data", response: {}, applied: false },
    { name: "undefined data", response: { error: undefined, data: undefined }, applied: false },
    { name: "successful empty data", response: { data: {} }, applied: true },
    { name: "newer busy revision", response: { data: {} }, applied: true, newer: true },
  ]) {
    const [state, set] = createEngineState()
    const list = [session("active"), session("unknown")]
    for (const info of list) putSession(set, info)
    set("status", "active", { type: "busy" })
    set("liveTools", "running-tool", "active")
    const captured = captureRevisions(state)
    if (scenario.newer)
      reduce(set, { type: "session.status", properties: { sessionID: "active", status: { type: "busy" } } } as never)
    const revisions = { ...state.revisions }
    let calls = 0
    reconcile(scenario.response, set, list, captured, (...args: Parameters<typeof applyStatusSnapshot>) => {
      calls++
      applyStatusSnapshot(...args)
    })

    const idle = scenario.applied && !scenario.newer
    expect({
      name: scenario.name,
      calls,
      active: state.status.active?.type,
      unknown: state.status.unknown?.type,
      liveTools: { ...state.liveTools },
      revisions: { ...state.revisions },
    }).toEqual({
      name: scenario.name,
      calls: Number(scenario.applied),
      active: idle ? "idle" : "busy",
      unknown: scenario.applied ? "idle" : undefined,
      liveTools: idle ? {} : { "running-tool": "active" },
      revisions,
    })
  }
})

test("a newer message-part event survives an older delayed transcript reload", async () => {
  let release!: () => void
  const pending = new Promise<void>((resolve) => (release = resolve))
  const { state, set, actions } = messagesHarness(async () => {
    await pending
    return { data: [entry("m1", "stale"), entry("m2")] }
  })
  putSession(set, session("session"))
  set("transcripts", "session", [entry("m1", "old")])

  const open = actions.openSession("session")
  reduce(set, {
    type: "message.part.updated",
    properties: { part: { id: "part-m1", sessionID: "session", messageID: "m1", type: "text", text: "newer" } },
  } as never)
  release()
  expect(await open).toBeTrue()
  const parts = state.transcripts.session?.find((item) => item.info.id === "m1")?.parts
  expect(parts?.[0]).toMatchObject({ id: "part-m1", text: "newer" })
  expect(state.transcripts.session?.map((item) => item.info.id)).toEqual(["m1", "m2"])
})

test("a message added by an event during hydration survives an older transcript snapshot", () => {
  const [state, set] = createEngineState()
  set("transcripts", "session", [entry("m1")])
  set("loaded", "session", true)
  const captured = captureRevisions(state)
  const fresh = entry("m2")
  reduce(set, { type: "message.updated", properties: { info: fresh.info } } as never)

  const merged = mergeTranscriptSnapshot(state.transcripts.session, [entry("m1")], "session", captured, state.revisions)
  expect(merged.map((item) => item.info.id)).toEqual(["m1", "m2"])
})

test("an unchanged transcript snapshot preserves live entry identity", () => {
  const [state, set] = createEngineState()
  const unchanged = entry("m1")
  const rewritten = entry("m2", "old")
  set("transcripts", "session", [unchanged, rewritten])
  set("loaded", "session", true)
  const captured = captureRevisions(state)

  const merged = mergeTranscriptSnapshot(
    state.transcripts.session,
    [entry("m1"), entry("m2", "new")],
    "session",
    captured,
    state.revisions,
  )
  // Same content keeps the same object so referentially-keyed rows do not remount on reconnect.
  expect(merged[0]).toBe(state.transcripts.session![0]!)
  // Changed content still takes the snapshot's newer entry.
  expect(merged[1]).not.toBe(state.transcripts.session![1]!)
  expect(merged[1]?.parts[0]).toMatchObject({ text: "new" })
})

test("a message removed by an event during hydration stays removed", () => {
  const [state, set] = createEngineState()
  set("transcripts", "session", [entry("m1"), entry("m2")])
  set("loaded", "session", true)
  const captured = captureRevisions(state)
  reduce(set, { type: "message.removed", properties: { sessionID: "session", messageID: "m2" } } as never)

  const merged = mergeTranscriptSnapshot(
    state.transcripts.session,
    [entry("m1"), entry("m2")],
    "session",
    captured,
    state.revisions,
  )
  expect(merged.map((item) => item.info.id)).toEqual(["m1"])
})

test("a transcript reload for a session deleted mid-flight is discarded", async () => {
  let release!: () => void
  const pending = new Promise<void>((resolve) => (release = resolve))
  const doomed = session("session")
  const { state, set, actions } = messagesHarness(async () => {
    await pending
    return { data: [entry("m1")] }
  })
  putSession(set, doomed)

  const open = actions.openSession("session")
  reduce(set, { type: "session.deleted", properties: { info: doomed } } as never)
  release()
  expect(await open).toBeTrue()
  expect(state.transcripts.session).toBeUndefined()
  expect(state.loaded.session).toBeUndefined()
})
