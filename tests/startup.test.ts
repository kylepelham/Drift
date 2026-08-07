import { expect, test } from "bun:test"
import type { Session } from "@opencode-ai/sdk/client"
import { createActions } from "../src/engine/actions"
import { createEngineState, putSessions } from "../src/engine/store"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

function session(id: string, fields: Partial<Session> = {}): Session {
  return {
    id,
    slug: id,
    projectID: "project",
    directory: "C:/work",
    title: id,
    version: "test",
    time: { created: 1, updated: 1 },
    ...fields,
  } as Session
}

test("bulk session hydration preserves updates and clears dropped optional fields", () => {
  const [state, set] = createEngineState()
  putSessions(set, [session("one", { revert: { messageID: "m1" }, share: { url: "https://example.test" } })])
  putSessions(set, [session("one", { title: "updated" }), session("two")])

  expect(Object.keys(state.sessions)).toEqual(["one", "two"])
  expect(state.sessions.one.title).toBe("updated")
  expect(state.sessions.one.revert).toBeUndefined()
  expect(state.sessions.one.share).toBeUndefined()
})

test("concurrent global session loads share one request", async () => {
  const [state, set] = createEngineState()
  const originalFetch = globalThis.fetch
  let requests = 0
  let release!: () => void
  const pending = new Promise<void>((resolve) => (release = resolve))
  globalThis.fetch = (async () => {
    requests += 1
    await pending
    return Response.json([session("one"), session("two")])
  }) as typeof fetch

  try {
    const actions = createActions(
      () => ({}) as never,
      state,
      set,
      () => ({ url: "http://engine.test" }),
    )
    const first = actions.loadAllSessions()
    const second = actions.loadAllSessions()
    release()
    await Promise.all([first, second])

    expect(requests).toBe(1)
    expect(Object.keys(state.sessions)).toEqual(["one", "two"])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("a complete global session load removes stale sessions and marks every workspace authoritative", async () => {
  const [state, set] = createEngineState()
  putSessions(set, [session("stale"), session("kept", { directory: "C:/other" })])
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async () => Response.json([session("kept", { directory: "C:/other" })])) as typeof fetch

  try {
    const actions = createActions(
      () => ({}) as never,
      state,
      set,
      () => ({ url: "http://engine.test" }),
    )
    await actions.loadAllSessions()

    expect(state.sessions.stale).toBeUndefined()
    expect(state.sessions.kept).toBeDefined()
    expect(state.sessionSnapshotAll).toBeTrue()
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("a global session load from an older connection cannot publish after reconnect", async () => {
  const [state, set] = createEngineState()
  let release!: () => void
  const pending = new Promise<void>((resolve) => (release = resolve))
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async () => {
    await pending
    return Response.json([])
  }) as typeof fetch

  try {
    const actions = createActions(
      () => ({}) as never,
      state,
      set,
      () => ({ url: "http://engine.test" }),
    )
    const load = actions.loadAllSessions()
    putSessions(set, [session("fresh")])
    set("sessionSnapshotEpoch", state.sessionSnapshotEpoch + 1)
    release()
    await load

    expect(state.sessions.fresh).toBeDefined()
    expect(state.sessionSnapshotAll).toBeFalse()
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("engine startup does not replace an active global event pump", async () => {
  const source = await Bun.file("src/engine/index.tsx").text()
  expect(source).toContain("if (!base || disposed || pumpAbort) return")
})

test("startup splash waits for workspace bootstrap without trapping empty or failed startup", async () => {
  const { startupReady } = await import("../src/ui/startup")
  const input = {
    workspacesReady: false,
    pluginsSettled: false,
    workspacePath: "C:/work",
    connection: "connecting" as const,
    bootstrappedDirectory: "",
    startupError: "",
  }

  expect(startupReady(input)).toBeFalse()
  expect(startupReady({ ...input, workspacesReady: true, workspacePath: null })).toBeFalse()
  expect(startupReady({ ...input, workspacesReady: true, pluginsSettled: true, workspacePath: null })).toBeTrue()
  expect(startupReady({ ...input, startupError: "engine failed" })).toBeTrue()
  expect(startupReady({ ...input, workspacesReady: true, pluginsSettled: true, connection: "online" })).toBeFalse()
  expect(
    startupReady({
      ...input,
      workspacesReady: true,
      pluginsSettled: true,
      connection: "online",
      bootstrappedDirectory: "C:/work",
    }),
  ).toBeTrue()
})

test("frontend mount removes the static first-paint placeholder", async () => {
  const [entry, document, styles] = await Promise.all([
    Bun.file("src/main.tsx").text(),
    Bun.file("index.html").text(),
    Bun.file("src/styles/app.css").text(),
  ])
  expect(document).toContain('class="drift-preload"')
  expect(document).toContain('localStorage.getItem("drift.theme")')
  expect(document).toContain('localStorage.getItem("drift.splash.enabled")')
  expect(document).toContain('localStorage.getItem("drift.splash.mascot")')
  expect(document).toContain("var(--bg, #141517)")
  expect(styles).toContain("color-mix(in srgb, var(--bg) 84%, var(--surface))")
  expect(styles).toContain("color: var(--accent)")
  expect(styles).toContain('[data-mascot="float"]')
  expect(styles).toContain('[data-exit="lift"]')
  expect(entry).toContain("root.replaceChildren()")
  expect(entry.indexOf("root.replaceChildren()")).toBeLessThan(entry.indexOf("render(() => <App />, root)"))
})
