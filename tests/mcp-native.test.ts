import { expect, test } from "bun:test"
import type { components } from "../src/engine/native/types"
import { registryConfig, registryServerName } from "../src/mcp-registry"
import { mcpConfigFromForm, mcpFormState, mcpRemoteUrlAllowed, updatePair } from "../src/state/mcp-form"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: { getItem: () => null, setItem: () => undefined },
  })

type McpServer = components["schemas"]["ServerStatus"]

function server(name: string, state: McpServer["state"]): McpServer {
  return { name, config: { type: "stdio", command: "npx", args: ["-y", "pkg@1.0.0"], env: [] }, enabled: state !== "disabled", updatedAt: 1, state, tools: [] }
}

test("the editor never holds a saved secret: untouched ones are kept by name, typed ones replace them", () => {
  const form = mcpFormState({ type: "stdio", command: "npx", args: ["-y", "pkg@1.0.0"], env: ["TOKEN", "MODE"] })
  expect(form.environment.every((pair) => pair.value === "" && pair.saved)).toBeTrue()
  const typed = { ...form, environment: updatePair(form.environment, 1, { value: "fast" }) }
  expect(mcpConfigFromForm(typed)).toEqual({ config: { type: "stdio", command: "npx", args: ["-y", "pkg@1.0.0"], env: { TOKEN: null, MODE: "fast" } } })
  const renamed = { ...form, environment: updatePair(form.environment, 0, { key: "API_TOKEN", value: "new" }) }
  expect(renamed.environment[0].saved).toBeFalse()
  const http = mcpFormState({ type: "http", url: "https://example.com/mcp", headers: ["Authorization"] })
  expect(mcpConfigFromForm(http)).toEqual({ config: { type: "http", url: "https://example.com/mcp", headers: { Authorization: null } } })
  expect(mcpConfigFromForm(mcpFormState())).toEqual({ issue: "commandRequired" })
})

test("the editor validates URLs and pairs", () => {
  const http = mcpFormState({ type: "http", url: "", headers: [] })
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

test("rows offer connect or disconnect only where the engine can do it", async () => {
  const { mcpRuntimeAction, mcpRuntimeKeyAction, nextMcpRowName } = await import("../src/ui/mcp/manager")
  expect(mcpRuntimeAction(server("a", "connected"))).toBe("disconnect")
  expect(mcpRuntimeAction(server("a", "failed"))).toBe("connect")
  expect(mcpRuntimeAction(server("a", "connecting"))).toBeUndefined()
  expect(mcpRuntimeAction(server("a", "disabled"))).toBeUndefined()
  expect(mcpRuntimeKeyAction(server("a", "connected"), "ArrowLeft")).toBe("disconnect")
  expect(mcpRuntimeKeyAction(server("a", "disconnected"), "ArrowRight")).toBe("connect")
  expect(nextMcpRowName(["a", "b", "c"], "c", "ArrowDown")).toBe("a")
})

test("dismissed notices are forgotten once they expire", async () => {
  const { pruneDismissedNoticeIds } = await import("../src/ui/notifications")
  expect(pruneDismissedNoticeIds(new Set(["kept", "expired"]), new Set(["kept", "new"]))).toEqual(new Set(["kept"]))
})
