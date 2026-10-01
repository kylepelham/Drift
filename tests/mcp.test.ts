// The frozen opencode approval plugin. The app's MCP servers live in the native engine (mcp-native.test.ts).
import { afterEach, describe, expect, test } from "bun:test"
import { mkdir, mkdtemp, readdir, rm, writeFile } from "node:fs/promises"
import os from "node:os"
import path from "node:path"
import { McpApproval } from "../engine/opencode/plugin/mcp-approval"

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

// Shared with src-tauri/src/mcp_external_tests.rs: the plugin and the Rust locator must hash this vector identically.
const externalParityFingerprint = "sha256:933d9f99f6458ef8004d9f0e9b5fe8768211fe67a62e7baa87b08d8e9a5220dd"

test("external fingerprint parity: the plugin and the Rust locator hash one vector identically", async () => {
  const setup = await fixture()
  const config = {
    mcp: {
      docs: {
        type: "remote",
        url: "https://example.com/mcp",
        headers: { Authorization: "Bearer x" },
        enabled: true,
        timeout: 30000,
      },
    },
  }
  await run("S:/repo", config, setup)
  const reported = await report(setup.pendingDirectory, "S:/repo")
  expect(reported.servers).toEqual([
    { name: "docs", type: "remote", fingerprint: externalParityFingerprint, decision: "pending" },
  ])
  const rust = await Bun.file("src-tauri/src/mcp_external_tests.rs").text()
  expect(rust).toContain(externalParityFingerprint)
})
