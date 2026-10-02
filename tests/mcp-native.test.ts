import { expect, test } from "bun:test"
import type { components } from "../src/engine/native/types"
import { preferredOption, registryConfig, registryInstallName, registryOptions, registryServerName } from "../src/mcp-registry"
import { createRegistrySearch, forgetRegistryCatalog, parseRegistryPayload, rankRegistry } from "../src/state/mcp-registry-search"
import { mcpConfigFromForm, mcpFormState, mcpRemoteUrlAllowed, updatePair } from "../src/state/mcp-form"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: { getItem: () => null, setItem: () => undefined },
  })

type McpServer = components["schemas"]["ServerStatus"]

function server(name: string, state: McpServer["state"]): McpServer {
  return { name, config: { type: "stdio", command: "npx", args: ["-y", "pkg@1.0.0"], env: [] }, enabled: state !== "disabled", updatedAt: 1, state, tools: [], transport: "stdio", needsSignIn: false, signedIn: false }
}

test("a server refusing until the user signs in says so instead of showing its raw error", async () => {
  const { mcpStatusLabel } = await import("../src/ui/mcp/manager")
  const refused = { ...server("secure", "failed"), error: "Auth required, when send initialize request" }
  expect(mcpStatusLabel(refused, false).text).toBe(refused.error)
  expect(mcpStatusLabel({ ...refused, needsSignIn: true }, false)).toEqual({ text: "sign-in required", tone: "text-warn" })
})

test("a server row says how it is spoken to, and the protocol version once connected", async () => {
  const { mcpProtocolLabel } = await import("../src/ui/mcp/manager")
  expect(mcpProtocolLabel({ transport: "stdio" })).toBe("stdio")
  expect(mcpProtocolLabel({ transport: "sse", protocol: "2024-11-05", era: "legacy" })).toBe("HTTP + SSE (deprecated) · 2024-11-05 · legacy")
  expect(mcpProtocolLabel({ transport: "streamable_http", protocol: "2026-07-28", era: "stateless" })).toBe("Streamable HTTP · 2026-07-28 · stateless")
})

test("the editor never holds a saved secret: untouched ones are kept by name, typed ones replace them", () => {
  const form = mcpFormState({ type: "stdio", command: "npx", args: ["-y", "pkg@1.0.0"], env: ["TOKEN", "MODE"] })
  expect(form.environment.every((pair) => pair.value === "" && pair.saved)).toBeTrue()
  const typed = { ...form, environment: updatePair(form.environment, 1, { value: "fast" }) }
  expect(mcpConfigFromForm(typed)).toEqual({ config: { type: "stdio", command: "npx", args: ["-y", "pkg@1.0.0"], env: { TOKEN: null, MODE: "fast" }, cwd: null, timeoutSeconds: null } })
  expect(mcpConfigFromForm({ ...typed, cwd: " C:/tools ", timeout: "90" }).config).toMatchObject({ cwd: "C:/tools", timeoutSeconds: 90 })
  expect(mcpConfigFromForm({ ...typed, timeout: "1.5" }).issue).toBe("timeoutInvalid")
  const legacy = mcpFormState({ type: "sse", url: "https://legacy.example/sse", headers: [], timeoutSeconds: 30 })
  expect(mcpConfigFromForm(legacy).config).toEqual({ type: "sse", url: "https://legacy.example/sse", headers: {}, oauth: null, timeoutSeconds: 30 })
  const renamed = { ...form, environment: updatePair(form.environment, 0, { key: "API_TOKEN", value: "new" }) }
  expect(renamed.environment[0].saved).toBeFalse()
  const http = mcpFormState({ type: "http", url: "https://example.com/mcp", headers: ["Authorization"] })
  expect(mcpConfigFromForm(http)).toEqual({ config: { type: "http", url: "https://example.com/mcp", headers: { Authorization: null }, oauth: null, timeoutSeconds: null } })
  const app = mcpFormState({ type: "http", url: "https://example.com/mcp", headers: [], oauth: { clientId: "team-app", hasSecret: true, scopes: ["read", "write"] } })
  expect([app.clientId, app.clientSecret, app.secretSaved, app.scopes]).toEqual(["team-app", "", true, "read write"])
  expect(mcpConfigFromForm(app).config).toMatchObject({ oauth: { clientId: "team-app", clientSecret: null, scopes: ["read", "write"] } })
  expect(mcpConfigFromForm({ ...app, clientSecret: "new", scopes: " read  " }).config).toMatchObject({ oauth: { clientId: "team-app", clientSecret: "new", scopes: ["read"] } })
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
          environmentVariables: [{ name: "TOKEN", isRequired: true, isSecret: true }, { name: "MODE", value: "fixed" }],
        },
      ],
    }),
  ).toBeNull()
  const [files] = registryOptions({
    name: "io.example/files",
    description: "Files",
    version: "1.2.3",
    packages: [
      {
        transport: { type: "stdio" },
        registryType: "npm",
        identifier: "@example/files",
        version: "1.2.3",
        packageArguments: [{ type: "named", name: "--root", default: "S:/repo" }],
        environmentVariables: [{ name: "TOKEN", isRequired: true, isSecret: true }, { name: "MODE", value: "fixed" }],
      },
    ],
  })
  expect(files.fields).toEqual([{ key: "env:TOKEN", label: "TOKEN", description: undefined, secret: true, required: true, default: undefined }])
  expect(files.build({ "env:TOKEN": " t0k " })).toEqual({
    type: "stdio",
    command: "npx",
    args: ["-y", "@example/files@1.2.3", "--root=S:/repo"],
    env: { TOKEN: "t0k", MODE: "fixed" },
  })
})

test("nothing is sent as an unexpanded placeholder: what an entry leaves open is asked for", () => {
  const [secret] = registryOptions({
    name: "io.example/secret",
    description: "Secret",
    version: "1.0.0",
    remotes: [{ type: "streamable-http", url: "https://example.com/mcp", headers: [{ name: "Authorization", isRequired: true, isSecret: true }] }],
  })
  expect(secret.build({})).toBeNull()
  expect(secret.build({ "header:Authorization": "abc" })).toEqual({ type: "http", url: "https://example.com/mcp", headers: { Authorization: "Bearer abc" } })
  expect(secret.build({ "header:Authorization": "Token abc" })?.headers).toEqual({ Authorization: "Token abc" })
  const [smithery] = registryOptions({
    name: "ai.smithery/x",
    description: "X",
    version: "1",
    remotes: [{ type: "streamable-http", url: "https://x.example/mcp", headers: [{ name: "Authorization", value: "Bearer {smithery_api_key}", isRequired: true, isSecret: true }] }],
  })
  expect(smithery.fields.map((field) => [field.label, field.secret, field.required])).toEqual([["smithery_api_key", true, true]])
  expect(smithery.build({ "header:Authorization:smithery_api_key": "k" })?.headers).toEqual({ Authorization: "Bearer k" })
  expect(
    registryConfig({
      name: "io.example/floating",
      description: "Floating",
      version: "1.0.0",
      packages: [{ transport: { type: "stdio" }, registryType: "npm", identifier: "floating", version: "latest" }],
    }),
  ).toEqual({ type: "stdio", command: "npx", args: ["-y", "floating@latest"], env: {} })
})

test("registry entries run as remotes over either transport, npx, uvx or docker, as the entry describes", () => {
  const github = registryOptions({
    name: "io.github.github/github-mcp-server",
    title: "GitHub",
    description: "GitHub",
    version: "1.13.0",
    remotes: [{ type: "streamable-http", url: "https://api.githubcopilot.com/mcp/", headers: [{ name: "Authorization", isSecret: true }] }],
    packages: [
      {
        transport: { type: "stdio" },
        registryType: "oci",
        identifier: "ghcr.io/github/github-mcp-server:1.13.0",
        version: "",
        runtimeArguments: [
          { type: "named", name: "-p", value: "127.0.0.1:8085:8085" },
          { type: "named", name: "-e", value: "GITHUB_PERSONAL_ACCESS_TOKEN={token}", variables: { token: { isSecret: true } } },
        ],
        environmentVariables: [{ name: "GITHUB_TOOLSETS", default: "repos" }],
      },
    ],
  })
  expect(github.map((option) => [option.kind, option.detail])).toEqual([["remote", "streamable-http"], ["docker", "1.13.0"]])
  expect(preferredOption(github)?.kind).toBe("remote")
  expect(github[0].build({})).toEqual({ type: "http", url: "https://api.githubcopilot.com/mcp/", headers: {} })
  expect(github[1].build({})).toEqual({
    type: "stdio",
    command: "docker",
    args: ["run", "-i", "--rm", "-e", "GITHUB_TOOLSETS", "-p", "127.0.0.1:8085:8085", "ghcr.io/github/github-mcp-server:1.13.0"],
    env: { GITHUB_TOOLSETS: "repos" },
  })
  const withToken = github[1].build({ "runtime:0:1:token": "pat" })
  expect(withToken?.args).toEqual(["run", "-i", "--rm", "-e", "GITHUB_TOOLSETS", "-e", "GITHUB_PERSONAL_ACCESS_TOKEN", "-p", "127.0.0.1:8085:8085", "ghcr.io/github/github-mcp-server:1.13.0"])
  expect(withToken?.type === "stdio" && withToken.env).toEqual({ GITHUB_TOOLSETS: "repos", GITHUB_PERSONAL_ACCESS_TOKEN: "pat" }, "a secret never sits in the arguments")
  expect(registryInstallName({ name: "io.github.github/github-mcp-server", title: "GitHub", description: "", version: "" })).toBe("github")
  expect(
    registryConfig({ name: "makenotion/notion-mcp-server", description: "Notion", version: "1.0.0", remotes: [{ type: "sse", url: "https://mcp.notion.com/sse" }] }),
  ).toEqual({ type: "sse", url: "https://mcp.notion.com/sse", headers: {} })
  const serena = registryConfig({
    name: "oraios/serena",
    description: "Serena",
    version: "1",
    packages: [
      {
        transport: { type: "stdio" },
        registryType: "pypi",
        identifier: "serena",
        version: "latest",
        runtimeHint: "uvx",
        runtimeArguments: [
          { type: "named", name: "--from", isRequired: true },
          { type: "positional", value: "git+https://github.com/oraios/serena", isRequired: true },
          { type: "positional", value: "serena", isRequired: true },
        ],
        packageArguments: [{ type: "named", name: "--context", isRequired: true }, { type: "positional", value: "ide-assistant", isRequired: true }],
      },
    ],
  })
  expect(serena).toEqual({ type: "stdio", command: "uvx", args: ["--from", "git+https://github.com/oraios/serena", "serena", "--context", "ide-assistant"], env: {} })
})

test("the catalog reads GitHub's listing, drops retired entries and keeps a server whose other package Drift cannot read", async () => {
  const github = {
    servers: [
      {
        server: {
          name: "microsoft/playwright-mcp",
          description: "Automate web browsers",
          version: "1",
          packages: [{ registryType: "npm", identifier: "@playwright/mcp", version: "latest", transport: { type: "stdio" } }, { registryType: "npm" }],
          repository: { url: "https://github.com/microsoft/playwright-mcp" },
          _meta: {
            "io.modelcontextprotocol.registry/publisher-provided": {
              github: { displayName: "Playwright", nameWithOwner: "microsoft/playwright-mcp", stargazerCount: 37751, preferredImage: "https://avatars.githubusercontent.com/u/1", topics: ["browser", "testing"] },
            },
          },
        },
      },
      { server: { name: "io.example/gone", description: "Gone", version: "1" }, _meta: { "io.modelcontextprotocol.registry/official": { status: "deleted" } } },
    ],
  }
  const [playwright, ...rest] = parseRegistryPayload(github, "github")
  expect(rest).toEqual([])
  expect(playwright.title).toBe("Playwright")
  expect(playwright.packages?.length).toBe(1)
  expect(playwright.listing).toMatchObject({ source: "github", publisher: "microsoft", stars: 37751, topics: ["browser", "testing"], repository: "https://github.com/microsoft/playwright-mcp" })
})

test("search ranks by title, then name, publisher, topics and description, popularity breaking ties", () => {
  const entry = (name: string, title: string, description: string, stars: number, topics: string[] = []) => ({
    name,
    title,
    description,
    version: "1",
    listing: { source: "github" as const, publisher: name.split("/")[0].split(".").at(-1), stars, topics },
  })
  const servers = [
    entry("io.github.netdata/mcp-server", "Netdata", "Monitoring", 80_000),
    entry("io.github.github/github-mcp-server", "GitHub", "Repos and issues", 30_000),
    entry("com.acme/browse", "Browser Kit", "Drives a browser", 10),
    entry("com.other/tools", "Toolbox", "Includes a browser", 5_000, ["browser"]),
  ]
  expect(rankRegistry(servers, "").map((s) => s.title)).toEqual(["Netdata", "GitHub", "Toolbox", "Browser Kit"])
  expect(rankRegistry(servers, "github").map((s) => s.title)).toEqual(["GitHub"])
  expect(rankRegistry(servers, "browser").map((s) => s.title)).toEqual(["Browser Kit", "Toolbox"])
  expect(rankRegistry(servers, "browser kit").map((s) => s.title)).toEqual(["Browser Kit"])
})

test("a search shows GitHub's popular servers, then official ones it lacks", async () => {
  const github = (name: string, repository: string) => ({ server: { name, description: name, version: "1", repository: { url: repository } } })
  const pages: Record<string, unknown> = {
    first: { servers: [github("io.example/alpha-docs", "https://github.com/example/alpha")], metadata: { nextCursor: "c2" } },
    second: { servers: [github("io.example/beta-docs", "https://github.com/example/beta")] },
    official: {
      servers: [
        { server: { name: "io.example/alpha-docs", description: "dup by name", version: "1" } },
        { server: { name: "io.mirror/alpha", description: "docs mirror", version: "1", repository: { url: "https://github.com/example/alpha" } } },
        { server: { name: "io.example/gamma-docs", description: "docs", version: "1" } },
      ],
    },
  }
  const asked: string[] = []
  const fetchRegistry = async (url: string) => {
    asked.push(url)
    const body = url.includes("registry.modelcontextprotocol.io") ? pages.official : url.includes("cursor=c2") ? pages.second : pages.first
    return { ok: true, json: async () => body }
  }
  forgetRegistryCatalog()
  const search = createRegistrySearch(fetchRegistry)
  const popular = await search.search("docs")
  expect(popular.servers.map((s) => s.name)).toEqual(["io.example/alpha-docs", "io.example/beta-docs"])
  const more = await search.official("docs", popular.servers)
  expect(more.servers.map((s) => s.name)).toEqual(["io.example/gamma-docs"])
  expect((await search.official("d", popular.servers)).servers).toEqual([])
  await search.search("")
  expect(asked.filter((url) => url.includes("api.mcp.github.com")).length).toBe(2)
  forgetRegistryCatalog()
})

test("cards say how a server runs and whether a key must be typed first", async () => {
  const { registryBadges } = await import("../src/ui/mcp/registry")
  const options = registryOptions({
    name: "io.example/keyed",
    description: "Keyed",
    version: "1.0.0",
    remotes: [{ type: "streamable-http", url: "https://example.com/mcp", headers: [{ name: "Authorization", isRequired: true }] }],
    packages: [{ transport: { type: "stdio" }, registryType: "pypi", identifier: "keyed", version: "1.0.0", environmentVariables: [{ name: "KEY", isRequired: true }] }],
  })
  expect(registryBadges(options)).toEqual(["Remote", "PyPI", "Needs a key"])
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
