import { expect, test } from "bun:test"
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import { pathToFileURL } from "node:url"
import { buildExtensions } from "../scripts/build-extensions"
import { SpawnThread } from "../engine/opencode/plugin/spawn-thread"
import { createOpencodeClient } from "@opencode-ai/sdk"
import type { PromptCatalog } from "../src/state/prompts"

const args = {
  title: "Child thread",
  task: "Investigate the failure",
  summary: "The parent encountered a failure.",
}

const context = {
  sessionID: "parent",
  messageID: "message",
  agent: "build",
  directory: "C:/workspace",
  worktree: "C:/workspace",
  abort: new AbortController().signal,
  metadata() {},
  async ask() {},
}

async function spawnTool(options?: {
  messages?: () => Promise<unknown>
  promptAsync?: (input: unknown) => Promise<unknown>
  message?: (input: unknown) => Promise<unknown>
  delete?: () => Promise<unknown>
}) {
  const deleted: string[] = []
  const prompted: unknown[] = []
  const client = {
    session: {
      async create() {
        return { data: { id: "child" } }
      },
      messages: options?.messages ?? (async () => ({ data: [] })),
      async promptAsync(input: unknown) {
        prompted.push(input)
        return options?.promptAsync ? options.promptAsync(input) : { data: undefined }
      },
      message: options?.message ?? (async () => ({ error: { message: "message not found" } })),
      delete:
        options?.delete ??
        (async ({ path: input }: { path: { id: string } }) => {
          deleted.push(input.id)
          return { data: true }
        }),
    },
  }
  const plugin = await SpawnThread({ client } as never)
  const execute = plugin.tool?.spawn_thread.execute
  if (!execute) throw new Error("spawn_thread tool was not registered")
  return { deleted, prompted, execute: () => execute(args, context) }
}

const spawnedPart = (id: string, status = "completed") => ({ type: "tool", tool: "spawn_thread", state: { status, metadata: { sessionId: id } } })

async function readTool(options: {
  parent?: unknown[]
  parentPages?: { data?: unknown[]; cursor?: string; error?: unknown }[]
  child?: unknown[]
  status?: Record<string, unknown>
  todos?: unknown[]
  permissions?: unknown[]
  questions?: unknown[]
} = {}) {
  const calls: string[] = []
  const parentQueries: { directory: string; limit?: number; before?: string }[] = []
  const client = {
    _client: {
      async get({ url }: { url: string }) {
        calls.push(url)
        return { data: url === "/permission" ? options.permissions ?? [] : options.questions ?? [] }
      },
    },
    session: {
      async messages({ path: input, query }: { path: { id: string }; query: { directory: string; limit?: number; before?: string } }) {
        calls.push(`messages:${input.id}`)
        if (input.id !== "parent") return { data: options.child ?? [] }
        parentQueries.push(query)
        const page = options.parentPages?.[parentQueries.length - 1]
        return {
          data: page ? page.data : options.parent ?? [{ info: { role: "assistant" }, parts: [spawnedPart("child")] }],
          error: page?.error,
          response: new Response(null, { headers: page?.cursor ? { "x-next-cursor": page.cursor } : {} }),
        }
      },
      async get() { return { data: { id: "child", title: "Child thread" } } },
      async status() { return { data: options.status ?? {} } },
      async todo() { return { data: options.todos ?? [] } },
    },
  }
  const plugin = await SpawnThread({ client } as never)
  const execute = plugin.tool?.read_thread.execute
  if (!execute) throw new Error("read_thread tool was not registered")
  const run = async (id = "child") => {
    const result = await execute({ id }, context)
    return typeof result === "string" ? result : result.output
  }
  return { calls, run, parentQueries }
}

test("read_thread searches older receipt pages and stops as soon as it finds the child", async () => {
  const view = await readTool({ parentPages: [
    { data: Array.from({ length: 50 }, () => ({ info: { role: "assistant" }, parts: [] })), cursor: "older-page" },
    { data: [{ info: { role: "assistant" }, parts: [spawnedPart("child")] }], cursor: "unneeded-page" },
  ] })
  expect(await view.run()).toContain('Thread "Child thread"')
  expect(view.parentQueries).toEqual([
    { directory: context.directory, limit: 50, before: undefined },
    { directory: context.directory, limit: 50, before: "older-page" },
  ])
})

test("read_thread scans to exhaustion before rejecting an unrelated child", async () => {
  const view = await readTool({ parentPages: [
    { data: [], cursor: "older-page" },
    { data: [{ info: { role: "assistant" }, parts: [spawnedPart("other")] }] },
  ] })
  await expect(view.run()).rejects.toThrow("was not spawned")
  expect(view.calls).toEqual(["messages:parent", "messages:parent"])
})

test("read_thread fails on page errors or repeating cursors without reading the child", async () => {
  for (const last of [{ error: { message: "history unavailable" } }, { data: [], cursor: "same" }]) {
    const view = await readTool({ parentPages: [{ data: [], cursor: "same" }, last] })
    await expect(view.run()).rejects.toThrow("Could not read spawn receipts")
    expect(view.calls).toEqual(["messages:parent", "messages:parent"])
  }
})

test("read_thread pagination survives the real SDK query serializer and preserves auth", async () => {
  const requests: URL[] = []
  const client = createOpencodeClient({
    baseUrl: "http://thread-test.invalid",
    headers: { Authorization: "Bearer fixture" },
    fetch: async (request) => {
      const req = request as Request
      const url = new URL(req.url)
      requests.push(url)
      expect(req.method).toBe("GET")
      expect(req.headers.get("authorization")).toBe("Bearer fixture")
      expect(url.searchParams.get("directory")).toBe(context.directory)
      if (url.pathname === "/session/parent/message") {
        expect(url.searchParams.get("limit")).toBe("50")
        if (!url.searchParams.has("before")) return Response.json([], { headers: { "X-Next-Cursor": "opaque+/=" } })
        expect(url.searchParams.get("before")).toBe("opaque+/=")
        return Response.json([{ info: { role: "assistant" }, parts: [spawnedPart("child")] }])
      }
      if (url.pathname === "/session/child") return Response.json({ title: "Child" })
      if (url.pathname === "/session/status") return Response.json({})
      return Response.json([])
    },
  })
  const plugin = await SpawnThread({ client } as never)
  const result = await plugin.tool!.read_thread!.execute({ id: "child" }, context)
  expect(result).toHaveProperty("output", expect.stringContaining("Status: idle"))
  expect(requests.filter((url) => url.pathname === "/session/parent/message")).toHaveLength(2)
})

test("read_thread refuses threads this conversation did not spawn", async () => {
  const other = await readTool()
  await expect(other.run("elsewhere")).rejects.toThrow("was not spawned from this conversation")
  expect(other.calls).toEqual(["messages:parent"])
  const failed = await readTool({ parent: [{ info: { role: "assistant" }, parts: [spawnedPart("child", "error")] }] })
  await expect(failed.run()).rejects.toThrow("was not spawned")
})

test("read_thread snapshots status, todos, recent tools and the latest reply without waiting", async () => {
  const view = await readTool({
    status: { child: { type: "busy" } },
    todos: [{ content: "Plan", status: "completed" }, { content: "Build", status: "in_progress" }, { content: "Ship", status: "pending" }],
    child: [
      { info: { role: "user" }, parts: [{ type: "text", text: "seed" }] },
      { info: { role: "assistant", time: { created: 0, completed: 1000 } }, parts: [
        { type: "reasoning", text: "PRIVATE_REASONING" },
        { type: "tool", tool: "bash", state: { status: "completed", title: "bun test", output: "SECRET_TOOL_OUTPUT" } },
        { type: "text", text: "Tests pass." },
      ] },
      { info: { role: "assistant" }, parts: [{ type: "tool", tool: "edit", state: { status: "running" } }] },
    ],
  })
  const output = await view.run()
  expect(output).toContain('Thread "Child thread" (child)')
  expect(output).toContain("Status: working")
  expect(output).toContain("- [x] Plan\n- [~] Build\n- [ ] Ship")
  expect(output).toContain("- bash completed: bun test\n- edit running")
  expect(output).toContain("Latest reply:\nTests pass.")
  expect(output).not.toContain("PRIVATE_REASONING")
  expect(output).not.toContain("SECRET_TOOL_OUTPUT")
})

test("read_thread reports pending approvals and questions for that thread only", async () => {
  const view = await readTool({
    status: { child: { type: "busy" } },
    permissions: [
      { sessionID: "child", permission: "bash", patterns: ["rm -rf dist"] },
      { sessionID: "someone-else", permission: "edit", patterns: ["x"] },
    ],
    questions: [{ sessionID: "child", questions: [{ question: "Which database?" }] }],
  })
  const output = await view.run()
  expect(output).toContain("waiting for the user to approve bash rm -rf dist")
  expect(output).toContain("waiting for the user to answer: Which database?")
  expect(output).not.toContain("edit x")
  expect(output).not.toContain("Status: working")
})

test("read_thread surfaces failures and caps long replies", async () => {
  const view = await readTool({ child: [
    { info: { role: "assistant" }, parts: [{ type: "text", text: "y".repeat(5000) }] },
    { info: { role: "assistant", error: { name: "APIError", data: { message: "rate limited" } } }, parts: [] },
  ] })
  const output = await view.run()
  expect(output).toContain("Status: stopped with an error: rate limited")
  expect(output).toContain(`${"y".repeat(4000)}\n[1000 more characters`)
})

test("read_thread bounds large snapshots and keeps only recent tool activity", async () => {
  const view = await readTool({
    todos: Array.from({ length: 100 }, (_, i) => ({ content: `${i} ${"x".repeat(1000)}`, status: "pending" })),
    child: [{ info: { role: "assistant" }, parts: [
      ...Array.from({ length: 30 }, (_, i) => ({ type: "tool", tool: `tool_${i}`, state: { status: "completed", title: "t".repeat(1000) } })),
      { type: "text", text: "answer ".repeat(1000) },
    ] }],
  })
  const output = await view.run()
  expect(output.length).toBeLessThanOrEqual(10000)
  expect(output).toContain("[80 more todos]")
  expect(output).toContain("- tool_20 completed")
  expect(output).not.toContain("- tool_19 completed")
  expect(output).not.toContain("x".repeat(201))
})

test("read_thread exposes retries and does not use synthetic text as a reply", async () => {
  const view = await readTool({
    status: { child: { type: "retry", attempt: 2, message: "Provider unavailable" } },
    child: [{ info: { role: "assistant" }, parts: [{ type: "text", text: "INTERNAL", synthetic: true }] }],
  })
  const output = await view.run()
  expect(output).toContain("retrying (attempt 2): Provider unavailable")
  expect(output).toContain("(no reply yet)")
  expect(output).not.toContain("INTERNAL")
})

test("spawn_thread reports only prompt admission after a successful 204", async () => {
  const spawn = await spawnTool()
  const result = await spawn.execute()
  expect(result).toMatchObject({
    metadata: { sessionId: "child", spawned: true },
  })
  expect(result).toHaveProperty("output", expect.stringContaining("seed prompt was accepted for processing"))
  expect(JSON.stringify(result)).not.toContain("working on the task")
  expect(spawn.deleted).toEqual([])
})

test("spawn_thread rejects SDK admission errors and removes the child", async () => {
  const spawn = await spawnTool({
    promptAsync: async () => ({ error: { data: { message: "model is unavailable" } } }),
  })
  await expect(spawn.execute()).rejects.toThrow("seed prompt was rejected: model is unavailable")
  expect(spawn.deleted).toEqual(["child"])
})

test("spawn_thread verifies admission after an ambiguous transport failure", async () => {
  let promptedMessageID = ""
  let verifiedMessageID = ""
  const spawn = await spawnTool({
    promptAsync: async (input) => {
      promptedMessageID = (input as { body: { messageID: string } }).body.messageID
      throw new Error("connection reset")
    },
    message: async (input) => {
      verifiedMessageID = (input as { path: { messageID: string } }).path.messageID
      return { data: { info: { id: verifiedMessageID, role: "user" }, parts: [] } }
    },
  })
  const result = await spawn.execute()
  expect(promptedMessageID).toStartWith("msg_")
  expect(verifiedMessageID).toBe(promptedMessageID)
  expect(result).toMatchObject({ metadata: { sessionId: "child", spawned: true } })
  expect(spawn.deleted).toEqual([])
})

test("spawn_thread preserves the child when transport admission remains unknown", async () => {
  const spawn = await spawnTool({
    promptAsync: async () => {
      throw new Error("connection reset")
    },
  })
  const error = await spawn.execute().catch((failure) => failure)
  expect(error).toBeInstanceOf(Error)
  expect(error.message).toContain("Admission is unknown and retryable; child session child was preserved")
  expect(error.message).toContain("Check for seed message msg_")
  expect(spawn.deleted).toEqual([])
})

test("spawn_thread cleans up when parent history retrieval throws before prompting", async () => {
  const spawn = await spawnTool({
    messages: async () => {
      throw new Error("history unavailable")
    },
  })
  await expect(spawn.execute()).rejects.toThrow(
    'Failed to prepare spawned thread "Child thread" before prompting: history unavailable.',
  )
  expect(spawn.prompted).toEqual([])
  expect(spawn.deleted).toEqual(["child"])
})

test("spawn_thread preserves preparation and cleanup error context", async () => {
  const spawn = await spawnTool({
    messages: async () => {
      throw new Error("history unavailable")
    },
    delete: async () => ({ error: { data: { message: "delete denied" } } }),
  })
  await expect(spawn.execute()).rejects.toThrow(
    'before prompting: history unavailable. Cleanup of child session child also failed: delete denied',
  )
})

test("release extensions load without workspace node_modules", async () => {
  const output = mkdtempSync(path.join(tmpdir(), "drift-extensions-"))
  try {
    await buildExtensions(output)
    const pluginPath = path.join(output, "plugin", "spawn-thread.js")
    const approvalPath = path.join(output, "plugin", "mcp-approval.js")
    const promptPath = path.join(output, "plugin", "prompt-overrides.js")
    const source = await Bun.file(pluginPath).text()
    const approvalSource = await Bun.file(approvalPath).text()
    const promptSource = await Bun.file(promptPath).text()
    const manifest = await Bun.file(path.join(output, "package.json")).json()
    expect(source).not.toContain('from"@opencode-ai/plugin"')
    expect(source).not.toContain('from"zod"')
    expect(approvalSource).not.toContain('from"@opencode-ai/plugin"')
    expect(promptSource).not.toContain('from"@opencode-ai/plugin"')
    expect(manifest.dependencies).toBeUndefined()
    expect(typeof (await import(pathToFileURL(pluginPath).href)).SpawnThread).toBe("function")
    const approval = await import(pathToFileURL(approvalPath).href)
    expect(typeof approval.McpApproval).toBe("function")
    expect(Object.values(approval).filter((value) => typeof value === "function")).toHaveLength(1)
    const prompt = await import(pathToFileURL(promptPath).href)
    const routing = await import(pathToFileURL(path.join(output, "tool-routing.js")).href)
    expect(typeof routing.routeTools).toBe("function")
    expect(typeof prompt.PromptOverrides).toBe("function")
    const catalog = await Bun.file(path.join(output, "prompt-catalog.json")).json()
    expect(catalog.families).toHaveLength(9)
    const gpt = catalog.families.find((item: { id: string }) => item.id === "gpt")
    expect(gpt.default).toStartWith("You are Drift")
    const settingsPath = path.join(output, "prompt-overrides.json")
    await Bun.write(settingsPath, JSON.stringify({ version: 1, families: {} }))
    const hooks = await prompt.PromptOverrides({} as never, {
      catalogPath: path.join(output, "prompt-catalog.json"),
      settingsPath,
    })
    const system = { system: [`${gpt.original}\nworkspace context`] }
    await hooks["experimental.chat.system.transform"]?.({ model: { api: { id: "gpt-5.4" } } } as never, system)
    expect(system.system[0]).toStartWith("You are Drift")
    expect(system.system[0]).toEndWith("workspace context")

    const anthropic = catalog.families.find((item: { id: string }) => item.id === "anthropic")
    const anthropicSystem = { system: [`${anthropic.original}\nworkspace context`] }
    await hooks["experimental.chat.system.transform"]?.(
      { model: { api: { id: "claude-opus-5" } } } as never,
      anthropicSystem,
    )
    const identity = anthropicSystem.system[0].split("\n\n", 1)[0]
    expect(identity).toStartWith("You are Drift")
    expect(identity).toContain("You are OpenCode")
    expect(anthropicSystem.system[0]).toEndWith("workspace context")

    const customAnthropic = "You are Drift for this workspace.\n\nKeep this custom first paragraph."
    await Bun.write(
      settingsPath,
      JSON.stringify({ version: 1, families: { anthropic: customAnthropic } }),
    )
    const customHooks = await prompt.PromptOverrides({} as never, {
      catalogPath: path.join(output, "prompt-catalog.json"),
      settingsPath,
    })
    const customSystem = { system: [`${anthropic.original}\nworkspace context`] }
    await customHooks["experimental.chat.system.transform"]?.(
      { model: { api: { id: "claude-opus-5" } } } as never,
      customSystem,
    )
    expect(customSystem.system[0]).toContain(customAnthropic)
    expect(customSystem.system[0]).toEndWith("workspace context")
  } finally {
    rmSync(output, { recursive: true, force: true })
  }
})

test("bundled GPT-6 prompts retain Astra defaults and existing GPT/Codex overrides", async () => {
  const output = mkdtempSync(path.join(tmpdir(), "drift-astra-extensions-"))
  try {
    await buildExtensions(output)
    const catalogPath = path.join(output, "prompt-catalog.json")
    const catalog: PromptCatalog = await Bun.file(catalogPath).json()
    const prompt = await import(pathToFileURL(path.join(output, "plugin", "prompt-overrides.js")).href)
    const settingsPath = path.join(output, "prompt-overrides.json")
    const astra = catalog.families.find((item) => item.id === "gpt")!.variants![0]
    expect(astra.id).toBe("gpt-astra")
    expect(astra.original).toBe(
      (await Bun.file("engine/upstream/packages/opencode/src/session/prompt/gpt-astra.txt").text()).trim(),
    )
    expect(astra.default).toBe(astra.original.replace(
      "You are an AI agent powered by OpenCode, a coding agent harness.",
      "You are Drift, an AI coding agent powered by OpenCode.",
    ))
    expect(catalog.families.find((item) => item.id === "codex")!.variants![0]).toEqual(astra)

    const settings: Record<string, string>[] = [
      {},
      { gpt: "Saved GPT instructions" },
      { codex: "Saved Codex instructions" },
      { gpt: "Saved GPT instructions", codex: "Saved Codex instructions" },
      { gpt: "", codex: "" },
      {}, // Reset removes overrides and restores model-specific defaults.
    ]
    const cases = [
      ["gpt-5.4", "gpt", false],
      ["gpt-5.3-codex", "codex", false],
      ["gpt-6", "gpt", true],
      ["gpt-6-astra", "gpt", true],
      ["gpt-6-mini", "gpt", true],
      ["gpt-6-codex", "codex", true],
    ] as const
    for (const families of settings) {
      await Bun.write(settingsPath, JSON.stringify({ version: 1, families }))
      const hooks = await prompt.PromptOverrides({} as never, { catalogPath, settingsPath })
      for (const [modelID, familyID, usesAstra] of cases) {
        const family = catalog.families.find((item) => item.id === familyID)!
        const template = usesAstra ? astra : family
        const suffix = "\n\nWorkspace rules\nSkills\nMCP instructions\nUser system text"
        const system = { system: [template.original + suffix, "Additional system message"] }
        await hooks["experimental.chat.system.transform"]({ model: { api: { id: modelID } } }, system)
        expect(system.system).toEqual([
          (families[familyID] ?? template.default) + suffix,
          "Additional system message",
        ])

        const agent = { system: ["Custom agent prompt\n" + template.original + suffix] }
        const original = [...agent.system]
        await hooks["experimental.chat.system.transform"]({ model: { api: { id: modelID } } }, agent)
        expect(agent.system).toEqual(original)
      }
    }
  } finally {
    rmSync(output, { recursive: true, force: true })
  }
})

test("release extension build removes stale raw resources from its output", async () => {
  const root = mkdtempSync(path.join(tmpdir(), "drift-release-extensions-"))
  const output = path.join(root, "generated")
  try {
    mkdirSync(path.join(output, "plugin"), { recursive: true })
    writeFileSync(path.join(output, "plugin", "spawn-thread.ts"), "stale")
    await buildExtensions(output)
    expect(await Bun.file(path.join(output, "plugin", "spawn-thread.ts")).exists()).toBe(false)
    expect(await Bun.file(path.join(output, "plugin", "spawn-thread.js")).exists()).toBe(true)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
