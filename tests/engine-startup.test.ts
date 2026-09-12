import { afterEach, expect, mock, test } from "bun:test"
import * as ts from "typescript"
import * as solid from "solid-js/dist/solid.js"
import { produce } from "solid-js/store"
import { applySessionSnapshot, applyStatusSnapshot } from "../src/engine/events"
import { captureRevisions, createEngineState, sessionSnapshotLimit } from "../src/engine/store"
import type { Engine } from "../src/engine"

const source = await Bun.file(new URL("../src/engine/index.tsx", import.meta.url)).text()
const parsed = ts.createSourceFile("index.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX)
// Execute the whole provider with controlled I/O, as in the lightbox tests. No global module
// mocks or copied startup logic; only JSX rendering and Vite's dev flag need substituting.
const executable = parsed.statements.filter((node) => !ts.isImportDeclaration(node))
  .map((node) => node.getText(parsed).replace(/^export /, "")).join("\n")
  .replaceAll("import.meta.env.DEV", "false")
const compiled = ts.transpileModule(executable, {
  fileName: "index.tsx",
  compilerOptions: {
    target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None, jsx: ts.JsxEmit.React, jsxFactory: "jsx",
  },
}).outputText

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}

async function settle() {
  // Drain the provider's promise continuations without waiting for its deliberately stalled I/O.
  await new Promise<void>((resolve) => setTimeout(resolve, 0))
}

const cleanups: (() => void)[] = []
afterEach(() => { for (const cleanup of cleanups.splice(0)) cleanup() })

function setup() {
  const [state, set] = createEngineState()
  const requests: ReturnType<typeof request>[] = []
  function request(directory: string) {
    const commands = deferred<{ data?: { name: string }[] }>()
    const config = deferred<{ data: { skills: { paths: string[] } } }>()
    const sessions = deferred<{ data: never[] }>()
    const api = {
      command: { list: mock(() => commands.promise) },
      config: { get: mock(() => config.promise) },
      session: { list: () => sessions.promise, status: async () => ({ data: {} }) },
      provider: { list: async () => ({ data: { all: [], connected: [], default: {} } }) },
      app: { agents: async () => ({ data: [{ name: "build" }] }) },
    }
    const result = { directory, commands, config, sessions, api }
    requests.push(result)
    return result
  }
  let active: ReturnType<typeof request>
  let connected!: () => void
  const health: { signal: AbortSignal; result: ReturnType<typeof deferred<Response>> }[] = []
  const timers = new Map<symbol, { callback: () => void; ms: number }>()
  const invoke = mock(async () => undefined)
  const dependencies = {
    ...solid, produce, createEngineState: () => [state, set], captureRevisions,
    applySessionSnapshot, applyStatusSnapshot, sessionSnapshotLimit,
    createOpencodeClient: ({ directory }: { directory: string }) => (active = request(directory)).api,
    createActions: () => ({ refreshPermissions: async () => undefined }),
    resolveEngine: async () => ({ url: "http://engine.test" }),
    restartShellEngine: async () => ({ url: "http://replacement.test" }),
    configureShellTimeout: async () => undefined,
    shellTimeoutMs: () => null, reportShellTimeoutError: () => {},
    seedProviderCatalog: () => {}, applyProviderCatalog: () => {},
    selectedSession: () => null, clearPermissionAttentionFor: () => {},
    shellInvoke: () => invoke, shellEvents: () => undefined,
    sleep: async () => {},
    fetch: (_url: string, { signal }: { signal: AbortSignal }) => {
      const result = deferred<Response>()
      health.push({ signal, result })
      return result.promise
    },
    setTimeout: (callback: () => void, ms: number) => {
      const id = Symbol()
      timers.set(id, { callback, ms })
      return id
    },
    clearTimeout: (id: symbol) => timers.delete(id),
    streamEvents: (_target: unknown, signal: AbortSignal, emit: (event: unknown) => void) => {
      connected = () => {
        // A reconnect keeps the SDK client, but makes a fresh pair of metadata requests.
        const next = request(active.directory)
        active.api.command.list.mockImplementation(() => next.commands.promise)
        active.api.config.get.mockImplementation(() => next.config.promise)
        active.api.session.list = () => next.sessions.promise
        emit({ type: "server.connected" })
      }
      emit({ type: "server.connected" })
      return new Promise<void>((resolve) => signal.addEventListener("abort", () => resolve(), { once: true }))
    },
    jsx: (_type: unknown, props: { value: Engine }) => props.value,
  }
  const run = new Function(...Object.keys(dependencies), `${compiled}\nreturn EngineProvider({});`)
  const root = solid.createRoot((dispose: () => void) => ({
    engine: run(...Object.values(dependencies)) as Engine, dispose,
  }))
  cleanups.push(root.dispose)
  root.engine.setDirectory("C:/work")
  return { ...root, state, set, requests, health, timers, invoke, reconnect: () => connected() }
}

test("startup and core hydration finish while MCP commands, config, and health are pending", async () => {
  const view = setup()
  await settle()
  expect(view.state.connection).toBe("online")
  expect(view.requests).toHaveLength(1)
  expect(view.health).toHaveLength(1)
  expect(view.state.bootstrappedDirectory).toBe("")

  view.requests[0].sessions.resolve({ data: [] })
  await settle()
  expect(view.state.bootstrappedDirectory).toBe("C:/work")
  expect(view.state.agents).toEqual([{ name: "build" }])

  view.requests[0].commands.resolve({ data: [{ name: "mcp:late" }] })
  await settle()
  expect(view.state.commands).toEqual([{ name: "mcp:late" }])
  expect(view.invoke).not.toHaveBeenCalled()
  view.requests[0].config.resolve({ data: { skills: { paths: ["skills"] } } })
  view.health[0].result.resolve(Response.json({ version: "1.2.3" }))
  await settle()
  expect(view.invoke).toHaveBeenCalledWith("watcher_set_skill_paths", { directory: "C:/work", paths: ["skills"] })
  expect(view.state.version).toBe("1.2.3")
  expect(view.timers.size).toBe(0)
})

test.each(["reject", "missing"])("optional metadata %s does not block readiness or erase commands", async (failure) => {
  const view = setup()
  view.set("commands", [{ name: "known" }] as never)
  await settle()
  const request = view.requests[0]
  if (failure === "reject") request.commands.reject(new Error("MCP unavailable"))
  else request.commands.resolve({})
  request.config.reject(new Error("config unavailable"))
  request.sessions.resolve({ data: [] })
  await settle()
  expect(view.state.bootstrappedDirectory).toBe("C:/work")
  expect(view.state.commands).toEqual([{ name: "known" }])
})

test.each(["workspace", "reconnect", "metadata"])("late hydration metadata cannot overwrite a newer %s refresh", async (change) => {
  const view = setup()
  await settle()
  const old = view.requests[0]
  if (change === "workspace") view.engine.setDirectory("C:/other")
  else if (change === "reconnect") view.reconnect()
  else {
    old.api.command.list.mockImplementation(async () => ({ data: [{ name: "fresh" }] }))
    old.api.config.get.mockImplementation(async () => ({ data: { skills: { paths: ["fresh"] } } }))
    await view.engine.refreshRuntimeMetadata()
  }
  if (change !== "metadata") {
    const fresh = view.requests[1]
    fresh.commands.resolve({ data: [{ name: "fresh" }] })
    fresh.config.resolve({ data: { skills: { paths: ["fresh"] } } })
    fresh.sessions.resolve({ data: [] })
  }
  await settle()
  old.commands.resolve({ data: [{ name: "stale" }] })
  old.config.resolve({ data: { skills: { paths: ["stale"] } } })
  old.sessions.resolve({ data: [] })
  await settle()
  expect(view.state.commands).toEqual([{ name: "fresh" }])
  expect(view.invoke).toHaveBeenCalledTimes(1)
  expect(view.invoke.mock.calls[0]).toEqual([
    "watcher_set_skill_paths", { directory: change === "workspace" ? "C:/other" : "C:/work", paths: ["fresh"] },
  ])
  expect(view.state.bootstrappedDirectory).toBe(change === "workspace" ? "C:/other" : "C:/work")
})

test("successful empty command metadata clears a previously populated list", async () => {
  const view = setup()
  view.set("commands", [{ name: "removed" }] as never)
  await settle()
  view.requests[0].commands.resolve({ data: [] })
  await settle()
  expect(view.state.commands).toEqual([])
})

test("version timeout aborts a stalled response body without delaying readiness or publishing it later", async () => {
  const view = setup()
  await settle()
  const body = deferred<{ version: string }>()
  view.health[0].result.resolve({ ok: true, json: () => body.promise } as Response)
  view.requests[0].sessions.resolve({ data: [] })
  await settle()
  expect(view.state.bootstrappedDirectory).toBe("C:/work")
  const [timer] = view.timers.values()
  expect(timer.ms).toBe(5_000)
  timer.callback()
  expect(view.health[0].signal.aborted).toBeTrue()
  body.resolve({ version: "late" })
  await settle()
  expect(view.state.version).toBe("")
  expect(view.timers.size).toBe(0)
})

test.each(["restart", "dispose"])("%s cancels health and rejects stale version and metadata updates", async (change) => {
  const view = setup()
  await settle()
  const old = view.requests[0]
  if (change === "restart") await view.engine.restartEngine()
  else view.dispose()
  expect(view.health[0].signal.aborted).toBeTrue()
  old.commands.resolve({ data: [{ name: "stale" }] })
  old.config.resolve({ data: { skills: { paths: ["stale"] } } })
  old.sessions.resolve({ data: [] })
  view.health[0].result.resolve(Response.json({ version: "stale" }))
  await settle()
  expect(view.state.commands).toEqual([])
  expect(view.state.version).toBe("")
  expect(view.invoke).not.toHaveBeenCalled()
  expect(view.state.bootstrappedDirectory).toBe("")
  if (change === "restart") {
    expect(view.health).toHaveLength(2)
    view.health[1].result.resolve(Response.json({ version: "replacement" }))
    await settle()
    expect(view.state.version).toBe("replacement")
  }
})

test.each(["restart", "reconnect", "restart without workspace"])("%s refreshes an already known engine version", async (change) => {
  const view = setup()
  await settle()
  view.health[0].result.resolve(Response.json({ version: "1.18.29" }))
  view.requests[0].sessions.resolve({ data: [] })
  await settle()
  expect(view.state.version).toBe("1.18.29")

  if (change === "reconnect") view.reconnect()
  else {
    if (change === "restart without workspace") view.engine.setDirectory(null)
    expect(await view.engine.restartEngine()).toBeTrue()
  }
  await settle()
  expect(view.health).toHaveLength(2)
  if (change !== "restart without workspace") {
    view.requests[1].sessions.resolve({ data: [] })
    await settle()
    expect(view.state.bootstrappedDirectory).toBe("C:/work")
    expect(view.state.connection).toBe("online")
  }
  // The version request can finish after readiness and must replace the previous value.
  view.health[1].result.resolve(Response.json({ version: "1.18.30" }))
  await settle()
  expect(view.state.version).toBe("1.18.30")
  expect(view.timers.size).toBe(0)
})
