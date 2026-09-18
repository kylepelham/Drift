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

async function preload(splash = true) {
  const document = await Bun.file("index.html").text()
  const script = [...document.matchAll(/<script>([\s\S]*?)<\/script>/g)].at(-1)![1]
  const commands: string[] = []
  const frames: (() => void)[] = []
  let decoded!: () => void
  const image = new Promise<void>((resolve) => { decoded = resolve })
  let revealed!: () => void
  const reveal = new Promise<void>((resolve) => { revealed = resolve })
  let publish!: (entries: { name: string }[]) => void
  let disconnected = false
  let images = 0
  const window = { __TAURI__: { core: { invoke: (command: string) => {
    commands.push(command)
    return reveal
  } } } } as unknown as Window & { __TAURI__: unknown }
  const observer = class {
    static supportedEntryTypes = ["paint"]
    constructor(callback: (list: { getEntries(): { name: string }[] }) => void) {
      publish = (entries) => callback({ getEntries: () => entries })
    }
    observe() {}
    disconnect() { disconnected = true }
  }
  Object.assign(window, { PerformanceObserver: observer })
  const run = new Function("window", "document", "requestAnimationFrame", "performance", "PerformanceObserver", "Image", script)
  run(window, {
    documentElement: { dataset: { splash: splash ? undefined : "hidden" } },
    getElementById: () => ({ href: "logo.svg" }),
  }, (callback: () => void) => frames.push(callback), { getEntriesByType: () => [], mark: () => {} }, observer, class {
    constructor() { images++ }
    decode() { return image }
  })
  const settle = async () => { for (let i = 0; i < 8; i++) await Promise.resolve() }
  const frame = async () => { frames.shift()?.(); await settle() }
  return { window, commands, frames, decoded, revealed, publish, frame, settle, disconnected: () => disconnected, images: () => images }
}

test("native reveal waits for contentful paint, decoded splash, and settled frames", async () => {
  const view = await preload()
  expect(view.commands).toEqual([])
  view.publish([{ name: "first-paint" }])
  view.decoded()
  await view.settle()
  expect(view.frames).toHaveLength(0)
  expect(view.commands).toEqual([])
  view.publish([{ name: "first-contentful-paint" }])
  await view.settle()
  expect(view.disconnected()).toBe(true)
  await view.frame()
  expect(view.commands).toEqual([])
  await view.frame()
  expect(view.commands).toEqual(["show_main_window"])
  let ready = false
  void view.window.__DRIFT_PRELOAD_READY__!.then(() => { ready = true })
  await view.settle()
  expect(ready).toBe(false)
  view.revealed()
  await view.settle()
  await view.frame()
  expect(ready).toBe(false)
  await view.frame()
  expect(ready).toBe(true)
})

test("a painted text splash cannot reveal before the logo finishes decoding", async () => {
  const view = await preload()
  view.publish([{ name: "first-contentful-paint" }])
  await view.settle()
  expect(view.frames).toHaveLength(0)
  expect(view.commands).toEqual([])
  view.decoded()
  await view.settle()
  await view.frame()
  await view.frame()
  expect(view.commands).toEqual(["show_main_window"])
  view.revealed()
  await view.settle()
  await view.frame()
  await view.frame()
})

test("disabled splash waits for app content to paint without loading the splash image", async () => {
  const view = await preload(false)
  expect(view.images()).toBe(0)
  expect(view.commands).toEqual([])
  view.publish([{ name: "first-contentful-paint" }])
  await view.settle()
  await view.frame()
  await view.frame()
  expect(view.commands).toEqual(["show_main_window"])
  view.revealed()
  await view.settle()
  await view.frame()
  await view.frame()
})

test("browser preload does not wait for a native reveal", async () => {
  const document = await Bun.file("index.html").text()
  const script = [...document.matchAll(/<script>([\s\S]*?)<\/script>/g)].at(-1)![1]
  new Function("window", script)({})
})

test("native startup keeps a bounded fallback for failed preload reveals", async () => {
  const source = await Bun.file(new URL("../src-tauri/src/main.rs", import.meta.url)).text()
  expect(source).toContain("tokio::time::sleep(std::time::Duration::from_secs(5))")
  expect(source).toContain("if !WINDOW_REVEALED.load")
  expect(source).toContain("reveal_main_window(&launch_window)")
})
