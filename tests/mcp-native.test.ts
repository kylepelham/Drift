import { expect, test } from "bun:test"
import type { components } from "../src/engine/native/types"
import { registryConfig, registryServerName } from "../src/mcp-registry"
import { mcpConfigFromForm, mcpFormState, mcpRemoteUrlAllowed } from "../src/state/mcp-form"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: { getItem: () => null, setItem: () => undefined },
  })

type McpServer = components["schemas"]["ServerStatus"]

function server(name: string, state: McpServer["state"], config: McpServer["config"] = { type: "stdio", command: "npx", args: ["-y", "pkg@1.0.0"] }): McpServer {
  return { name, config, enabled: true, updatedAt: 1, state, tools: [] }
}

test("the editor round-trips exactly the engine's config, nothing it cannot run", () => {
  const stdio = { type: "stdio" as const, command: "npx", args: ["-y", "pkg@1.0.0"], env: { TOKEN: "abc" } }
  expect(mcpConfigFromForm(mcpFormState(stdio))).toEqual({ config: stdio })
  const http = { type: "http" as const, url: "https://example.com/mcp", headers: { Authorization: "Bearer x" } }
  expect(mcpConfigFromForm(mcpFormState(http))).toEqual({ config: http })
  expect(mcpConfigFromForm(mcpFormState())).toEqual({ issue: "commandRequired" })
})

test("the editor validates URLs and pairs", () => {
  const http = mcpFormState({ type: "http", url: "" })
  expect(mcpConfigFromForm(http)).toEqual({ issue: "urlRequired" })
  expect(mcpConfigFromForm({ ...http, url: "ftp://example.com" })).toEqual({ issue: "urlInvalid" })
  const duplicate = [
    { key: "X", value: "1" },
    { key: "X", value: "2" },
  ]
  expect(mcpConfigFromForm({ ...http, url: "https://example.com", headers: duplicate })).toEqual({ issue: "pairInvalid" })
  expect(mcpRemoteUrlAllowed("http://127.0.0.1:8765/mcp")).toBeTrue()
})

test("registry entries become engine configs, with names a tool name can carry", () => {
  expect(registryServerName("io.example/docs.v2")).toBe("docs-v2")
  expect(
    registryConfig({
      name: "io.example/docs",
      description: "Docs",
      version: "1.0.0",
      remotes: [{ type: "streamable-http", url: "https://example.com/{tenant}", variables: { tenant: { value: "mcp" } }, headers: [{ name: "X-Mode", value: "fast" }] }],
    }),
  ).toEqual({ type: "http", url: "https://example.com/mcp", headers: { "X-Mode": "fast" } })
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
          environmentVariables: [{ name: "TOKEN", isRequired: true }, { name: "MODE", value: "fixed" }],
        },
      ],
    }),
  ).toEqual({ type: "stdio", command: "npx", args: ["-y", "@example/files@1.2.3", "--root=S:/repo"], env: { MODE: "fixed" } })
})

test("nothing is sent as an unexpanded placeholder: a remote needing an unknown header is skipped", () => {
  expect(
    registryConfig({
      name: "io.example/secret",
      description: "Secret",
      version: "1.0.0",
      remotes: [{ type: "streamable-http", url: "https://example.com/mcp", headers: [{ name: "Authorization", isRequired: true }] }],
    }),
  ).toBeNull()
  expect(
    registryConfig({
      name: "io.example/floating",
      description: "Floating",
      version: "1.0.0",
      packages: [{ transport: { type: "stdio" }, registryType: "npm", identifier: "floating", version: "latest" }],
    }),
  ).toBeNull()
})

test("approval prompts list servers awaiting approval and change key when the definition changes", async () => {
  const { mcpCommandLine, mcpPromptKey, mcpPromptTargets } = await import("../src/ui/notifications")
  const pending = server("files", "needs_approval")
  const targets = mcpPromptTargets({ docs: server("docs", "connected"), files: pending })
  expect(targets.map((target) => target.name)).toEqual(["files"])
  expect(mcpCommandLine(pending)).toBe("npx -y pkg@1.0.0")
  const changed = server("files", "needs_approval", { type: "stdio", command: "node", args: [] })
  expect(mcpPromptKey(changed)).not.toBe(mcpPromptKey(pending))
})

test("rows offer connect or disconnect only where the engine can do it", async () => {
  const { mcpRuntimeAction, mcpRuntimeKeyAction, nextMcpRowName } = await import("../src/ui/mcp/manager")
  expect(mcpRuntimeAction(server("a", "connected"))).toBe("disconnect")
  expect(mcpRuntimeAction(server("a", "failed"))).toBe("connect")
  expect(mcpRuntimeAction(server("a", "needs_approval"))).toBeUndefined()
  expect(mcpRuntimeAction(server("a", "disabled"))).toBeUndefined()
  expect(mcpRuntimeKeyAction(server("a", "connected"), "ArrowLeft")).toBe("disconnect")
  expect(mcpRuntimeKeyAction(server("a", "disconnected"), "ArrowRight")).toBe("connect")
  expect(nextMcpRowName(["a", "b", "c"], "c", "ArrowDown")).toBe("a")
})

test("notice ids stay distinct per occurrence and dismissed ones are pruned", async () => {
  const { nextNoticeOccurrenceId, pruneDismissedNoticeIds } = await import("../src/ui/notifications")
  const first = nextNoticeOccurrenceId("failure")
  const second = nextNoticeOccurrenceId("failure")
  expect(second).not.toBe(first)
  expect(pruneDismissedNoticeIds(new Set([first, "expired"]), new Set([first, second]))).toEqual(new Set([first]))
})
