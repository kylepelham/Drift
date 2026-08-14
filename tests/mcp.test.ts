import { afterEach, describe, expect, test } from "bun:test"
import { mkdir, mkdtemp, readdir, rm, writeFile } from "node:fs/promises"
import os from "node:os"
import path from "node:path"
import { McpApproval } from "../engine/opencode/plugin/mcp-approval"
import { registryConfig } from "../src/mcp-registry"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: { getItem: () => null, setItem: () => undefined },
  })

const roots: string[] = []
const gate = Symbol.for("drift.mcp.approval.gate")

afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })))
})

async function fixture(generation = 1, decisions: Array<{ fingerprint: string; decision: string }> = []) {
  const root = await mkdtemp(path.join(os.tmpdir(), "drift-mcp-test-"))
  roots.push(root)
  const policyPath = path.join(root, "policy.json")
  const pendingDirectory = path.join(root, "pending")
  const sentinelPath = path.join(root, "mcp-fail-closed.json")
  await Bun.write(policyPath, JSON.stringify({ version: 3, generation, decisions }))
  return { root, policyPath, pendingDirectory, sentinelPath, generation }
}

async function run(
  directory: string,
  config: Record<string, unknown>,
  settings: { policyPath: string; pendingDirectory: string; sentinelPath: string; generation: number },
) {
  const plugin = await McpApproval({ directory } as never, settings)
  await plugin.config?.(config as never)
  return config
}

async function report(pendingDirectory: string, directory?: string) {
  const files = await readdir(pendingDirectory)
  const reports = await Promise.all(files.map((file) => Bun.file(path.join(pendingDirectory, file)).json()))
  const result = directory ? reports.find((item) => item.directory === directory) : reports.at(-1)
  expect(result).toBeDefined()
  return result
}

describe("global MCP approval gate", () => {
  test("removes the sidecar password before MCP children can inherit it", async () => {
    const setup = await fixture()
    const previous = process.env.OPENCODE_SERVER_PASSWORD
    process.env.OPENCODE_SERVER_PASSWORD = "must-not-reach-mcp"
    await McpApproval({ directory: "S:/repo" } as never, setup)
    expect(process.env.OPENCODE_SERVER_PASSWORD).toBeUndefined()
    if (previous !== undefined) process.env.OPENCODE_SERVER_PASSWORD = previous
  })

  test("fails closed for missing policy, malformed entries, and report failures", async () => {
    const setup = await fixture()
    await rm(setup.policyPath)
    const missing = {
      mcp: {
        local: { type: "local", command: ["dangerous"] },
        malformed: { command: ["also-dangerous"] },
      },
    }
    await run("S:/repo", missing, setup)
    expect(missing.mcp.local).toEqual({ enabled: false })
    expect(missing.mcp.malformed).toEqual({ enabled: false })
    expect((missing as Record<PropertyKey, unknown>)[gate]).toBeFunction()
    expect(((missing as Record<PropertyKey, unknown>)[gate] as (value: unknown) => boolean)(missing)).toBeTrue()

    await Bun.write(setup.policyPath, JSON.stringify({ version: 3, generation: 2, decisions: [] }))
    const stale = { mcp: { local: { type: "local", command: ["dangerous"] } } }
    await run("S:/repo", stale, setup)
    expect(stale.mcp.local).toEqual({ enabled: false })

    await Bun.write(setup.policyPath, JSON.stringify({ version: 3, generation: 1, decisions: [], unexpected: true }))
    const malformed = { mcp: { local: { type: "local", command: ["dangerous"] } } }
    await run("S:/repo", malformed, setup)
    expect(malformed.mcp.local).toEqual({ enabled: false })

    await rm(setup.pendingDirectory, { recursive: true, force: true })
    await writeFile(setup.pendingDirectory, "not a directory")
    await Bun.write(setup.policyPath, JSON.stringify({ version: 3, generation: 1, decisions: [] }))
    const unwritable = { mcp: { local: { type: "local", command: ["dangerous"] } } }
    await run("S:/repo", unwritable, setup)
    expect(unwritable.mcp.local).toEqual({ enabled: false })
  })

  test("isolates invalid effective external transports without hiding valid servers", async () => {
    const setup = await fixture()
    const config = {
      mcp: {
        empty: { type: "local", command: [] },
        unsupported: { type: "remote", url: "ftp://example.com", headers: { Authorization: "secret" } },
        http: { type: "remote", url: "http://192.0.2.10:8765/mcp" },
        valid: { type: "remote", url: "https://valid.example.com" },
      },
    }
    await run("S:/repo", config, setup)
    expect(config.mcp).toEqual({
      empty: { enabled: false },
      unsupported: { enabled: false },
      http: { enabled: false },
      valid: { enabled: false },
    })
    const invalid = await report(setup.pendingDirectory, "S:/repo")
    expect(invalid.servers.map((server: { name: string; decision: string }) => [server.name, server.decision])).toEqual([
      ["empty", "invalid"],
      ["unsupported", "invalid"],
      ["http", "pending"],
      ["valid", "pending"],
    ])
    expect(JSON.stringify(invalid)).not.toContain("secret")
  })

  test("does not corrupt cached source definitions while filtering MCPs", async () => {
    const setup = await fixture()
    const source = {
      docs: { type: "remote" as const, url: "https://example.com/mcp" },
      malformed: { command: ["unsafe"] },
    }
    const config = { mcp: source }
    await run("S:/repo", config, setup)
    expect(config.mcp).not.toBe(source)
    expect(config.mcp).toEqual({ docs: { enabled: false }, malformed: { enabled: false } })
    expect(source).toEqual({
      docs: { type: "remote", url: "https://example.com/mcp" },
      malformed: { command: ["unsafe"] },
    })
  })

  test("fail-closed sentinel overrides an otherwise approved policy", async () => {
    const setup = await fixture(3)
    const definition = { type: "remote" as const, url: "https://example.com" }
    await run("S:/repo", { mcp: { docs: definition } }, setup)
    const fingerprint = (await report(setup.pendingDirectory, "S:/repo")).servers[0].fingerprint
    await Bun.write(
      setup.policyPath,
      JSON.stringify({ version: 3, generation: 3, decisions: [{ fingerprint, decision: "approved" }] }),
    )
    await Bun.write(setup.sentinelPath, JSON.stringify({ version: 1, failClosed: true }))
    const config = { mcp: { docs: definition } }
    await run("S:/repo", config, setup)
    expect(config.mcp.docs).toEqual({ enabled: false })
    expect((await report(setup.pendingDirectory, "S:/repo")).servers).toEqual([])
  })

  test("uses one exact fingerprint globally and excludes only enabled", async () => {
    const setup = await fixture(4)
    const definition = {
      type: "remote" as const,
      url: "https://example.com/mcp",
      headers: { Authorization: "Bearer expanded-secret" },
      oauth: { clientSecret: "expanded-oauth-secret", callbackPort: 29418 },
      timeout: 90_000,
      unknown: { cwd: "effective-custom-value" },
      enabled: false,
    }
    await run("S:/one", { mcp: { docs: definition } }, setup)
    const first = await report(setup.pendingDirectory, "S:/one")
    const fingerprint = first.servers[0].fingerprint as string

    const reordered = {
      mcp: {
        docs: {
          enabled: false,
          unknown: { cwd: "effective-custom-value" },
          timeout: 90_000,
          oauth: { callbackPort: 29418, clientSecret: "expanded-oauth-secret" },
          headers: { Authorization: "Bearer expanded-secret" },
          url: "https://example.com/mcp",
          type: "remote" as const,
        },
      },
    }
    await run("S:/one", reordered, setup)
    expect((await report(setup.pendingDirectory, "S:/one")).servers[0].fingerprint).toBe(fingerprint)

    await run("S:/one", { mcp: { renamed: definition } }, setup)
    expect((await report(setup.pendingDirectory, "S:/one")).servers[0].fingerprint).not.toBe(fingerprint)

    await Bun.write(
      setup.policyPath,
      JSON.stringify({ version: 3, generation: 4, decisions: [{ fingerprint, decision: "approved" }] }),
    )
    const otherDirectory = { mcp: { docs: { ...definition, enabled: true } } }
    await run("S:/two", otherDirectory, setup)
    expect(otherDirectory.mcp.docs).toEqual({ ...definition, enabled: true })
    expect((await report(setup.pendingDirectory, "S:/two")).servers[0]).toEqual({
      name: "docs",
      type: "remote",
      fingerprint,
      decision: "approved",
    })

    const changedValues = [
      { ...definition, url: "https://other.example/mcp" },
      { ...definition, headers: { Authorization: "different" } },
      { ...definition, oauth: { clientSecret: "different", callbackPort: 29418 } },
      { ...definition, timeout: 90_001 },
      { ...definition, unknown: { cwd: "changed" } },
    ]
    for (const changed of changedValues) {
      const config = { mcp: { docs: changed } }
      await run("S:/two", config, setup)
      expect(config.mcp.docs).toEqual({ enabled: false })
      expect((await report(setup.pendingDirectory, "S:/two")).servers[0].fingerprint).not.toBe(fingerprint)
    }

    const returned = { mcp: { docs: { ...definition } } }
    await run("S:/three", returned, setup)
    expect(returned.mcp.docs).toEqual(definition)
  })

  test("persists exact rejection without exposing secrets in reports", async () => {
    const setup = await fixture(7)
    const definition = {
      type: "local" as const,
      command: ["secret-command", "secret-argument"],
      cwd: "secret-directory",
      environment: { SECRET_NAME: "secret-value" },
      unknown: { token: "secret-custom" },
    }
    await run("S:/repo", { mcp: { private: definition } }, setup)
    const pending = await report(setup.pendingDirectory, "S:/repo")
    const fingerprint = pending.servers[0].fingerprint
    expect(Object.keys(pending).sort()).toEqual(["directory", "generation", "servers", "version"])
    expect(Object.keys(pending.servers[0]).sort()).toEqual(["decision", "fingerprint", "name", "type"])
    expect(JSON.stringify(pending)).not.toContain("secret-")

    await Bun.write(
      setup.policyPath,
      JSON.stringify({ version: 3, generation: 7, decisions: [{ fingerprint, decision: "rejected" }] }),
    )
    const rejected = { mcp: { private: definition } }
    await run("S:/other", rejected, setup)
    expect(rejected.mcp.private).toEqual({ enabled: false })
    expect((await report(setup.pendingDirectory, "S:/other")).servers[0].decision).toBe("rejected")

    const changed = { mcp: { private: { ...definition, command: ["new-command"] } } }
    await run("S:/other", changed, setup)
    expect((await report(setup.pendingDirectory, "S:/other")).servers[0].decision).toBe("pending")
  })

  test("seal detects MCP mutation after the approval hook", async () => {
    const setup = await fixture()
    const config = { mcp: { docs: { type: "remote", url: "https://example.com" } } }
    await run("S:/repo", config, setup)
    const verify = (config as Record<PropertyKey, unknown>)[gate] as (value: unknown) => boolean
    const descriptor = Object.getOwnPropertyDescriptor(config, gate)
    expect(descriptor?.configurable).toBeFalse()
    expect(descriptor?.writable).toBeFalse()
    expect(() => Object.defineProperty(config, gate, { value: () => true })).toThrow()
    expect(verify(config)).toBeTrue()
    config.mcp.docs = { type: "remote", url: "https://attacker.example" } as never
    expect(verify(config)).toBeFalse()
  })

  test("Windows report paths use ASCII-only case folding", async () => {
    if (process.platform !== "win32") return
    const setup = await fixture()
    const definition = { type: "remote" as const, url: "https://example.com" }
    await run("S:\\Ünicode\\İ\\Repo\\", { mcp: { docs: definition } }, setup)
    await run("s:/Ünicode/İ/repo", { mcp: { docs: definition } }, setup)
    expect(await readdir(setup.pendingDirectory)).toHaveLength(1)
    await run("s:/ünicode/İ/repo", { mcp: { docs: definition } }, setup)
    expect(await readdir(setup.pendingDirectory)).toHaveLength(2)
  })

  test("failed report replacement removes its temporary file", async () => {
    const setup = await fixture()
    const definition = { type: "remote" as const, url: "https://example.com" }
    await run("S:/repo", { mcp: { docs: definition } }, setup)
    const destination = path.join(setup.pendingDirectory, (await readdir(setup.pendingDirectory))[0])
    await rm(destination)
    await mkdir(destination)
    const config = { mcp: { docs: definition } }
    await run("S:/repo", config, setup)
    expect(config.mcp.docs).toEqual({ enabled: false })
    expect((await readdir(setup.pendingDirectory)).some((file) => file.endsWith(".tmp"))).toBeFalse()
  })
})

test("browser MCP registry fails clearly instead of emulating policy", async () => {
  if (!("localStorage" in globalThis)) {
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      value: { getItem: () => null, setItem: () => undefined },
    })
  }
  const { driftStore } = await import("../src/state/store")
  await expect(driftStore.mcpSnapshot("S:/repo")).rejects.toThrow("desktop backend")
})

describe("MCP frontend coordinator", () => {
  const dependencies = (store: Record<string, unknown>, extra: Record<string, unknown> = {}) => ({
    store: store as never,
    status: async () => ({}),
    connect: async () => undefined,
    disconnect: async () => undefined,
    authenticate: async () => undefined,
    ...extra,
  })

  test("start loads global servers without an active workspace", async () => {
    const { createMcpCoordinator } = await import("../src/state/mcp")
    const directories: string[] = []
    const coordinator = createMcpCoordinator()
    const stop = coordinator.start(
      dependencies({
        mcpSnapshot: async (directory: string) => {
          directories.push(directory)
          return {
            generation: 2,
            directory,
            servers: [{ name: "global", config: { type: "local", command: ["server"] }, updatedAt: 1 }],
            observed: [],
          }
        },
      }),
    )
    await coordinator.settled()
    expect(directories).toEqual([""])
    expect(coordinator.state.snapshot.servers[0].name).toBe("global")
    expect(coordinator.state.ready).toBeTrue()
    stop()
  })

  test("initializes the engine before reading the effective snapshot", async () => {
    const { createMcpCoordinator } = await import("../src/state/mcp")
    const order: string[] = []
    const coordinator = createMcpCoordinator(
      dependencies(
        {
          mcpSnapshot: async (directory: string) => {
            order.push("snapshot")
            return { generation: 3, directory, servers: [], observed: [] }
          },
        },
        {
          status: async () => {
            order.push("status")
            return {}
          },
        },
      ),
    )
    await coordinator.setActive("S:/repo", true)
    expect(order).toEqual(["status", "snapshot"])
    expect(coordinator.state.snapshot.generation).toBe(3)
  })

  test("rejects stale exact actions without falling back to a server name", async () => {
    const { createMcpCoordinator, exactMcpTarget } = await import("../src/state/mcp")
    let generation = 4
    let fingerprint = "sha256:first"
    const approvals: unknown[][] = []
    const store = {
      mcpSnapshot: async (directory: string) => ({
        generation,
        directory,
        servers: [],
        observed: [{ name: "docs", type: "remote" as const, fingerprint, decision: "pending" as const }],
      }),
      approveMcp: async (...args: unknown[]) => {
        approvals.push(args)
      },
    }
    const coordinator = createMcpCoordinator(dependencies(store))
    await coordinator.setActive("S:/repo", true)
    const target = exactMcpTarget(coordinator.state.snapshot, coordinator.state.snapshot.observed[0])
    generation = 5
    fingerprint = "sha256:changed"
    await coordinator.refresh()
    await expect(coordinator.decide("approve", target)).rejects.toThrow("stale")
    expect(approvals).toEqual([])
  })

  test("workspace failure clears old rows immediately and leaves nothing actionable", async () => {
    const { createMcpCoordinator } = await import("../src/state/mcp")
    const coordinator = createMcpCoordinator(
      dependencies({
        mcpSnapshot: async (directory: string) => {
          if (directory === "S:/broken") throw new Error("report failed")
          return {
            generation: 4,
            directory,
            servers: [],
            observed: [{ name: "docs", type: "remote", fingerprint: "sha256:one", decision: "approved" }],
          }
        },
      }),
    )
    await coordinator.setActive("S:/one", true)
    const changing = coordinator.setActive("S:/broken", true)
    expect(coordinator.state.snapshot.directory).toBe("S:/broken")
    expect(coordinator.state.snapshot.observed).toEqual([])
    expect(coordinator.state.ready).toBeFalse()
    await expect(changing).rejects.toThrow("report failed")
    expect(coordinator.state.snapshot.observed).toEqual([])
    expect(coordinator.state.error).toBe("report failed")
  })

  test("runtime actions require the captured exact target after workspace switches", async () => {
    const { createMcpCoordinator, exactMcpTarget } = await import("../src/state/mcp")
    const connects: string[] = []
    const coordinator = createMcpCoordinator(
      dependencies(
        {
          mcpSnapshot: async (directory: string) => ({
            generation: 8,
            directory,
            servers: [],
            observed: [{ name: "docs", type: "remote", fingerprint: `sha256:${directory}`, decision: "approved" }],
          }),
        },
        {
          connect: async (name: string) => {
            connects.push(name)
          },
        },
      ),
    )
    await coordinator.setActive("S:/one", true)
    const target = exactMcpTarget(coordinator.state.snapshot, coordinator.state.snapshot.observed[0])
    const switched = coordinator.setActive("S:/two", true)
    await expect(coordinator.runtime(target, "connect")).rejects.toThrow(/refreshing|stale/)
    await switched
    expect(connects).toEqual([])
  })

  test("editor expectations detect updatedAt and generation conflicts", async () => {
    const { createMcpCoordinator } = await import("../src/state/mcp")
    let generation = 2
    let updatedAt = 10
    let saves = 0
    const coordinator = createMcpCoordinator(
      dependencies({
        mcpSnapshot: async (directory: string) => ({
          generation,
          directory,
          servers: [{ name: "docs", config: { type: "remote", url: "https://example.com" }, updatedAt }],
          observed: [],
        }),
        saveMcp: async () => {
          saves++
        },
      }),
    )
    await coordinator.setActive("", false)
    await coordinator.refresh()
    const expected = { generation: 2, previousName: "docs", updatedAt: 10 }
    updatedAt = 11
    await coordinator.refresh()
    await expect(
      coordinator.save("docs", { type: "remote", url: "https://changed.example" }, expected),
    ).rejects.toThrow("changed")
    generation = 3
    await coordinator.refresh()
    await expect(
      coordinator.save("docs", { type: "remote", url: "https://changed.example" }, expected),
    ).rejects.toThrow("changed")
    expect(saves).toBe(0)
  })

  test("mutation lock is synchronous and rejects double or cross-row submissions", async () => {
    const { createMcpCoordinator } = await import("../src/state/mcp")
    let release!: () => void
    const pending = new Promise<void>((resolve) => {
      release = resolve
    })
    const coordinator = createMcpCoordinator(
      dependencies({
        mcpSnapshot: async (directory: string) => ({ generation: 3, directory, servers: [], observed: [] }),
        saveMcp: async () => pending,
      }),
    )
    await coordinator.refresh()
    const first = coordinator.save("one", { type: "local", command: ["one"] }, { generation: 3 })
    expect(coordinator.state.mutation).toBe("one")
    await expect(coordinator.save("two", { type: "local", command: ["two"] }, { generation: 3 })).rejects.toThrow(
      "already in progress",
    )
    release()
    await first
    expect(coordinator.state.mutation).toBeNull()
  })

  test("committed mutations remain successful when only the follow-up refresh fails", async () => {
    const { createMcpCoordinator } = await import("../src/state/mcp")
    let snapshots = 0
    let saves = 0
    const coordinator = createMcpCoordinator(
      dependencies({
        mcpSnapshot: async (directory: string) => {
          if (snapshots++) throw new Error("refresh failed")
          return { generation: 3, directory, servers: [], observed: [] }
        },
        saveMcp: async () => {
          saves++
        },
      }),
    )
    await coordinator.refresh()
    await coordinator.save("docs", { type: "local", command: ["docs"] }, { generation: 3 })
    expect(saves).toBe(1)
    expect(coordinator.state.mutation).toBeNull()
    expect(coordinator.state.error).toBe("refresh failed")
    expect(coordinator.state.ready).toBeFalse()
  })

  test("queued refreshes skip contexts superseded before they start", async () => {
    const { createMcpCoordinator } = await import("../src/state/mcp")
    const directories: string[] = []
    const coordinator = createMcpCoordinator()
    coordinator.start(
      dependencies({
        mcpSnapshot: async (directory: string) => {
          directories.push(directory)
          return { generation: 1, directory, servers: [], observed: [] }
        },
      }),
    )
    await coordinator.setActive("S:/current", true)
    expect(directories).toEqual(["S:/current"])
  })

  test("status-only refresh keeps the actionable snapshot while updating runtime state", async () => {
    const { createMcpCoordinator } = await import("../src/state/mcp")
    let snapshots = 0
    let statuses = 0
    const coordinator = createMcpCoordinator(
      dependencies(
        {
          mcpSnapshot: async (directory: string) => {
            snapshots++
            return { generation: 6, directory, servers: [], observed: [] }
          },
        },
        {
          status: async () => {
            statuses++
            if (statuses === 1) return { docs: { status: "failed" as const, error: "closed" } }
            if (statuses === 2) return { docs: { status: "connected" as const } }
            return {}
          },
        },
      ),
    )
    await coordinator.setActive("S:/repo", true)
    await coordinator.refreshStatus()
    expect(snapshots).toBe(1)
    expect(statuses).toBe(2)
    expect(coordinator.state.statuses.docs.status).toBe("connected")
    expect(coordinator.state.ready).toBeTrue()
    expect(coordinator.state.loading).toBeFalse()
    await coordinator.refreshStatus()
    expect(coordinator.state.statuses).toEqual({})
  })

  test("failed status polling clears stale connected state", async () => {
    const { createMcpCoordinator } = await import("../src/state/mcp")
    let fail = false
    const coordinator = createMcpCoordinator(
      dependencies(
        {
          mcpSnapshot: async (directory: string) => ({ generation: 1, directory, servers: [], observed: [] }),
        },
        {
          status: async () => {
            if (fail) throw new Error("engine unavailable")
            return { docs: { status: "connected" } }
          },
        },
      ),
    )
    await coordinator.setActive("S:/repo", true)
    expect(coordinator.state.statuses.docs.status).toBe("connected")

    fail = true
    await expect(coordinator.refreshStatus()).rejects.toThrow("engine unavailable")
    expect(coordinator.state.statuses).toEqual({})
    expect(coordinator.state.ready).toBeTrue()
  })

  test("external refresh bursts coalesce and cancellation prevents delayed work", async () => {
    const { createMcpRefreshDebouncer } = await import("../src/state/mcp")
    const queued = new Map<number, () => void>()
    let id = 0
    let runs = 0
    const debouncer = createMcpRefreshDebouncer(() => runs++, 100, {
      set: (callback) => {
        queued.set(++id, callback)
        return id as ReturnType<typeof setTimeout>
      },
      clear: (timer) => queued.delete(timer as number),
    })
    debouncer.trigger()
    debouncer.trigger()
    debouncer.trigger()
    expect(queued.size).toBe(1)
    const scheduled = queued.entries().next().value
    if (scheduled) {
      queued.delete(scheduled[0])
      scheduled[1]()
    }
    expect(runs).toBe(1)
    debouncer.trigger()
    debouncer.cancel()
    expect(queued.size).toBe(0)
  })
})

describe("MCP editor round trip", () => {
  test("preserves unknown top-level and OAuth fields", async () => {
    const { mcpConfigFromForm, mcpFormState } = await import("../src/state/mcp-form")
    const config = {
      type: "remote" as const,
      url: "https://example.com/mcp",
      headers: { Authorization: "Bearer {env:TOKEN}" },
      enabled: false,
      timeout: 9000,
      oauth: {
        clientId: "client",
        clientSecret: "{env:SECRET}",
        scope: "read write",
        callbackPort: 29418,
        redirectUri: "http://127.0.0.1:29418/callback",
        vendorOAuth: { audience: "docs" },
      },
      vendorOption: { mode: "strict" },
    }
    expect(mcpConfigFromForm(mcpFormState(config))).toEqual({ config })
  })

  test("validates local commands, remote URLs, pairs, and callback ports", async () => {
    const { mcpConfigFromForm, mcpFormState, mcpRemoteUrlAllowed } = await import("../src/state/mcp-form")
    expect(mcpConfigFromForm(mcpFormState({ type: "local", command: [] }))).toEqual({ issue: "commandRequired" })
    const remote = mcpFormState({ type: "remote", url: "http://example.com" })
    expect(mcpConfigFromForm(remote)).toEqual({ config: { type: "remote", url: "http://example.com" } })
    expect(
      mcpConfigFromForm({ ...remote, url: "https://example.com", callbackPort: "65536", oauthMode: "configured" }),
    ).toEqual({ issue: "callbackPortInvalid" })
    expect(
      mcpConfigFromForm({
        ...remote,
        url: "https://example.com",
        headers: [
          { key: "X", value: "1" },
          { key: "X", value: "2" },
        ],
      }),
    ).toEqual({ issue: "pairInvalid" })
    expect(mcpRemoteUrlAllowed("http://127.42.8.9/mcp")).toBeTrue()
    expect(mcpRemoteUrlAllowed("http://[::1]/mcp")).toBeTrue()
    expect(mcpRemoteUrlAllowed("http://192.0.2.10:8765/mcp")).toBeTrue()
    expect(mcpRemoteUrlAllowed("ftp://example.com/mcp")).toBeFalse()
  })
})

describe("official MCP registry conversion", () => {
  test("prefers HTTPS remotes and builds placeholders", () => {
    expect(
      registryConfig({
        name: "io.example/docs",
        description: "Docs",
        version: "1.0.0",
        remotes: [
          {
            type: "streamable-http",
            url: "https://example.com/{tenant}",
            variables: { tenant: { value: "mcp" } },
            headers: [{ name: "TOKEN" }],
          },
        ],
      }),
    ).toEqual({ type: "remote", url: "https://example.com/mcp", headers: { TOKEN: "{env:TOKEN}" } })
  })

  test("accepts only pinned npm and PyPI stdio packages", () => {
    expect(
      registryConfig({
        name: "io.example/files",
        description: "Files",
        version: "1.2.3",
        packages: [
          {
            transport: { type: "stdio" },
            registryType: "npm",
            identifier: "@example/files",
            version: "1.2.3",
            runtimeHint: "npx",
            runtimeArguments: [{ type: "named", name: "-y", value: "" }],
            packageArguments: [{ type: "named", name: "--root", default: "S:/repo" }],
        environmentVariables: [
          { name: "TOKEN" },
          { name: "MODE", value: "fixed" },
          { name: "ENDPOINT", value: "https://{host}", variables: { host: { default: "api.example" } } },
        ],
          },
        ],
      }),
    ).toEqual({
      type: "local",
      command: ["npx", "-y", "@example/files@1.2.3", "--root=S:/repo"],
      environment: { TOKEN: "{env:TOKEN}", MODE: "fixed", ENDPOINT: "https://api.example" },
    })
    expect(
      registryConfig({
        name: "io.example/floating",
        description: "Floating",
        version: "1.0.0",
        packages: [{ transport: { type: "stdio" }, registryType: "npm", identifier: "floating", version: "latest" }],
      }),
    ).toBeNull()
    expect(
      registryConfig({
        name: "io.example/python",
        description: "Python",
        version: "2.1.0",
        packages: [
          { transport: { type: "stdio" }, registryType: "pypi", identifier: "python-mcp", version: "2.1.0rc1" },
        ],
      }),
    ).toEqual({ type: "local", command: ["uvx", "python-mcp==2.1.0rc1"] })
    expect(
      registryConfig({
        name: "io.example/unsafe",
        description: "Unsafe",
        version: "1.0.0",
        packages: [{ transport: { type: "stdio" }, registryType: "npm", identifier: "--require", version: "1.0.0" }],
      }),
    ).toBeNull()
    expect(
      registryConfig({
        name: "io.example/options",
        description: "Options",
        version: "1.0.0",
        packages: [
          {
            transport: { type: "stdio" },
            registryType: "npm",
            identifier: "safe",
            version: "1.0.0",
            packageArguments: [{ type: "positional", value: "--exec" }],
          },
        ],
      }),
    ).toBeNull()
  })

  test("validates payloads and discards stale registry responses", async () => {
    const { createRegistrySearch, parseRegistryPayload } = await import("../src/state/mcp-registry-search")
    expect(() => parseRegistryPayload({ servers: "bad" })).toThrow("invalid response")
    expect(parseRegistryPayload({ servers: [{ server: { name: "bad", description: "x", version: "1" } }] })).toEqual([])
    const payload = {
      servers: [
        {
          server: {
            name: "io.example/docs",
            description: "Docs",
            version: "1.0.0",
            remotes: [{ type: "streamable-http", url: "https://example.com/mcp" }],
          },
        },
      ],
    }
    const pending: Array<(value: { ok: boolean; json: () => Promise<unknown> }) => void> = []
    const search = createRegistrySearch(async () => new Promise((resolve) => pending.push(resolve)))
    const first = search.search("first")
    const second = search.search("second")
    pending[1]({ ok: true, json: async () => payload })
    expect(await second).toMatchObject({ stale: false })
    pending[0]({ ok: true, json: async () => payload })
    expect(await first).toEqual({ stale: true, servers: [] })
    search.dispose()
  })
})

test("MCP notification actions keep other prompts visible throughout refresh", async () => {
  const { mcpPromptKey, mcpPromptTargets, reduceMcpPromptState } = await import("../src/ui/notifications")
  const snapshot = {
    generation: 4,
    directory: "S:/repo",
    servers: [],
    observed: [
      { name: "one", type: "remote" as const, fingerprint: "sha256:one", decision: "pending" as const },
      { name: "two", type: "local" as const, fingerprint: "sha256:two", decision: "pending" as const },
    ],
  }
  const targets = mcpPromptTargets({ directory: "S:/repo", snapshot })
  expect(targets).toHaveLength(2)
  const first = mcpPromptKey(targets[0])
  const started = reduceMcpPromptState(new Set(), { type: "start", key: first })
  expect(targets.filter((target) => !started.has(mcpPromptKey(target))).map((target) => target.name)).toEqual(["two"])
  const failed = reduceMcpPromptState(started, { type: "failed", key: first })
  expect(failed.has(first)).toBeFalse()
  expect(mcpPromptTargets({ directory: "S:/other", snapshot })).toEqual([])
})

test("notification stack stays below the titlebar and scrolls when crowded", async () => {
  const source = await Bun.file("src/ui/notifications.tsx").text()
  expect(source).toContain("fixed top-11 right-5 bottom-5")
  expect(source).toContain("max-h-full min-h-0 w-full flex-col gap-2 overflow-y-auto")
})

test("only the MCP row owning a runtime mutation shows loading", async () => {
  const source = await Bun.file("src/ui/mcp/manager.tsx").text()
  expect(source).toContain("busy={coordinator.state.mutation === name}")
  expect(source).not.toContain("busy={!!coordinator.state.mutation}")
})

test("MCP status refreshes keep focused rows mounted by stable server name", async () => {
  const source = await Bun.file("src/ui/mcp/manager.tsx").text()
  expect(source).toContain("<For each={rowNames()}>")
  expect(source).not.toContain("<For each={rows()}>")
})

test("MCP keyboard navigation selects rows and maps transport controls", async () => {
  const { mcpRuntimeAction, mcpRuntimeKeyAction, nextMcpRowName } = await import("../src/ui/mcp/manager")
  const names = ["alpha", "bravo", "charlie"]
  expect(nextMcpRowName(names, "bravo", "ArrowDown")).toBe("charlie")
  expect(nextMcpRowName(names, "charlie", "ArrowDown")).toBe("alpha")
  expect(nextMcpRowName(names, "alpha", "ArrowUp")).toBe("charlie")
  expect(nextMcpRowName(names, "bravo", "Home")).toBe("alpha")
  expect(nextMcpRowName(names, "bravo", "End")).toBe("charlie")

  expect(mcpRuntimeAction({ status: "connected" })).toBe("disconnect")
  expect(mcpRuntimeAction({ status: "failed", error: "closed" })).toBe("connect")
  expect(mcpRuntimeAction({ status: "needs_auth" })).toBe("authenticate")
  expect(mcpRuntimeKeyAction({ status: "connected" }, "ArrowLeft")).toBe("disconnect")
  expect(mcpRuntimeKeyAction({ status: "connected" }, "ArrowRight")).toBeUndefined()
  expect(mcpRuntimeKeyAction({ status: "failed", error: "closed" }, "ArrowRight")).toBe("connect")
  expect(mcpRuntimeKeyAction({ status: "failed", error: "tool failed" }, "ArrowLeft")).toBeUndefined()
})

test("repeated notice occurrences remain visible and dismissed ids are pruned", async () => {
  const { nextNoticeOccurrenceId, pruneDismissedNoticeIds } = await import("../src/ui/notifications")
  const first = nextNoticeOccurrenceId("failure")
  const second = nextNoticeOccurrenceId("failure")
  expect(second).not.toBe(first)
  expect(pruneDismissedNoticeIds(new Set([first, "expired"]), new Set([first, second]))).toEqual(new Set([first]))
})

test("MCP SDK helpers propagate error responses and missing data", async () => {
  const { requireSdkData } = await import("../src/engine/actions")
  expect(() => requireSdkData({ error: { data: { message: "transport failed" } } }, "fallback")).toThrow(
    "transport failed",
  )
  expect(() => requireSdkData({}, "missing response")).toThrow("missing response")
  expect(requireSdkData({ data: { docs: { status: "connected" } } }, "fallback")).toEqual({
    docs: { status: "connected" },
  })
})
