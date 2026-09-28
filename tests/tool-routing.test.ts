import { expect, mock, test } from "bun:test"
import { createToolRouter, type Status } from "../engine/opencode/tool-routing"

type TestTool = { description: string; execute?: () => Promise<unknown> }
const tools: Record<string, TestTool> = {
  read: { description: "Read a file" }, bash: { description: "Run a command" },
  docs_search: { description: "Search documentation" }, docs_fetch: { description: "Read a page" },
  github_issues: { description: "List GitHub issues" }, github_pr: { description: "Read a PR" },
}
function setup(scores: unknown = { g0: { type: "noul", noul: 0.99 }, g1: { type: "noul", noul: 0.01 } }) {
  let enabled = true
  const fetcher = mock(async (_url: string, _init: RequestInit) => Response.json({ answers: scores }))
  const credential = mock(async (providerID: string): Promise<string | undefined> => providerID === "opencode" ? "test-key" : undefined)
  const reports: Status[] = []
  const router = createToolRouter({
    policy: async () => ({ enabled }), fetch: fetcher, timeoutMs: 25, report: (status) => reports.push(status),
  })
  const input = {
    tools, servers: ["docs", "github"], sessionID: "s1", turnID: "u1", abort: new AbortController().signal,
    messages: [{ role: "user", content: "Look up Solid documentation" }], credential,
    expandTool: (execute: () => Promise<unknown>): TestTool => ({ description: "Expand tools", execute }),
  }
  const outcomes = () => reports.map((status) => status.outcome)
  return { router, input, fetcher, credential, reports, outcomes, enable: (value: boolean) => { enabled = value } }
}

test("off makes no routing request or credential read", async () => {
  const view = setup()
  view.enable(false)
  expect(await view.router(view.input)).toBe(tools)
  expect(view.fetcher).not.toHaveBeenCalled()
  expect(view.credential).not.toHaveBeenCalled()
  expect(view.reports).toEqual([])
})

test("shortlists groups without removing core tools and recovery expands on the next step", async () => {
  const view = setup()
  const routed = await view.router(view.input)
  expect(Object.keys(routed)).toEqual(["read", "bash", "docs_search", "docs_fetch", "drift_expand_tools"])
  expect(routed.read).toBe(tools.read)
  expect(tools).not.toHaveProperty("drift_expand_tools")
  await routed.drift_expand_tools!.execute!()
  expect(await view.router(view.input)).toBe(tools)
  expect(view.fetcher).toHaveBeenCalledTimes(1)
})

test("requests use Zen auth and batched independent relevance questions", async () => {
  const view = setup()
  await view.router(view.input)
  const [url, init] = view.fetcher.mock.calls[0]!
  expect(url).toBe("https://opencode.ai/zen/v1/systemone")
  expect(init!.headers).toMatchObject({ Authorization: "Bearer test-key" })
  expect(init!.redirect).toBe("error")
  const body = JSON.parse(init!.body as string)
  expect(body.model).toBe("jev-1.13")
  expect(Object.keys(body.questions)).toEqual(["g0", "g1"])
  expect(body.questions.g0.type).toBe("noul")
  expect(body.state.context).toEqual(view.input.messages.map((m) => ({ role: m.role, text: m.content })))
})

test("one routing evaluation per session turn, new turn and catalog changes invalidate it", async () => {
  const view = setup()
  await Promise.all([view.router(view.input), view.router(view.input)])
  expect(view.fetcher).toHaveBeenCalledTimes(1)
  await view.router({ ...view.input, turnID: "u2" })
  await view.router({ ...view.input, sessionID: "s2" })
  await view.router({ ...view.input, tools: { ...tools, docs_new: { description: "A new operation" } } })
  expect(view.fetcher).toHaveBeenCalledTimes(4)
})

test("schema omission and permission filtering are preserved when expanding", async () => {
  const view = setup()
  const allowed = { ...tools }
  delete allowed.github_pr
  const input = { ...view.input, tools: allowed }
  const routed = await view.router(input)
  await routed.drift_expand_tools!.execute!()
  expect(await view.router(input)).toBe(allowed)
  expect(await view.router(input)).not.toHaveProperty("github_pr")
})

test("previously used tools remain exposed for history replay", async () => {
  const view = setup()
  const input = { ...view.input, messages: [
    ...view.input.messages, { role: "assistant", content: [{ type: "tool-call", toolName: "github_pr" }] },
  ] }
  expect(await view.router(input)).toHaveProperty("github_pr")
})

test.each([
  undefined, {},
  { g0: { type: "choice", noul: 0.99 }, g1: { type: "noul", noul: 0.01 } },
  { g0: { type: "noul", noul: 2 }, g1: { type: "noul", noul: 0.01 } },
])("malformed responses keep all tools: %j", async (scores) => {
  const view = setup(scores === undefined ? null : scores)
  expect(await view.router(view.input)).toBe(tools)
  expect(view.outcomes()).toEqual(["invalid-response"])
})

test("only confidently irrelevant groups are hidden, unsure groups stay", async () => {
  const view = setup({ g0: { type: "noul", noul: 0.5 }, g1: { type: "noul", noul: 0.15 } })
  const routed = await view.router(view.input)
  expect(Object.keys(routed)).toEqual(["read", "bash", "docs_search", "docs_fetch", "drift_expand_tools"])
})

test("when no group is clearly unrelated every tool stays and the turn reports uncertain", async () => {
  const view = setup({ g0: { type: "noul", noul: 0.2 }, g1: { type: "noul", noul: 0.61 } })
  expect(await view.router(view.input)).toBe(tools)
  expect(view.outcomes()).toEqual(["uncertain"])
})

test("many relevant groups no longer cancel routing", async () => {
  const servers = ["a", "b", "c", "d", "e", "f"]
  const many = Object.fromEntries(servers.map((server) => [`${server}_tool`, { description: server }]))
  const scores = Object.fromEntries(servers.map((_, index) => [`g${index}`, { type: "noul", noul: index === 5 ? 0.01 : 0.9 }]))
  const view = setup(scores)
  const routed = await view.router({ ...view.input, servers, tools: many })
  expect(Object.keys(routed)).toEqual(["a_tool", "b_tool", "c_tool", "d_tool", "e_tool", "drift_expand_tools"])
})

test("absent credentials, HTTP failures, and network failures keep all tools and report why", async () => {
  const view = setup()
  view.credential.mockImplementation(async () => undefined)
  expect(await view.router(view.input)).toBe(tools)
  expect(view.fetcher).not.toHaveBeenCalled()
  view.credential.mockImplementation(async () => "test-key")
  const statuses = [[401, "u2"], [402, "u3"], [429, "u4"]] as const
  for (const [status, turnID] of statuses) {
    view.fetcher.mockImplementation(async () => new Response(null, { status }))
    expect(await view.router({ ...view.input, turnID })).toBe(tools)
  }
  view.fetcher.mockImplementation(async () => { throw new Error("offline") })
  expect(await view.router({ ...view.input, turnID: "u5" })).toBe(tools)
  expect(view.outcomes()).toEqual(["no-key", "unauthorized", "insufficient-funds", "http-error", "network"])
  expect(view.reports[3]!.httpStatus).toBe(429)
})

test("an OpenCode Go key is used when no Zen key is stored", async () => {
  const view = setup()
  view.credential.mockImplementation(async (providerID) => providerID === "opencode-go" ? "go-key" : undefined)
  await view.router(view.input)
  expect(view.credential.mock.calls.map(([providerID]) => providerID)).toEqual(["opencode", "opencode-go"])
  expect(view.fetcher.mock.calls[0]![1]!.headers).toMatchObject({ Authorization: "Bearer go-key" })
})

test("each turn reports once, with the hidden tool count, across model steps", async () => {
  const view = setup()
  await view.router(view.input)
  await view.router(view.input)
  expect(view.reports).toEqual([{ outcome: "routed", hidden: 2, at: expect.any(Number) }])
  const status = JSON.stringify(view.reports[0])
  expect(status).not.toContain("Solid")
  expect(status).not.toContain("test-key")
})

test("catalogs routing cannot help report a single skip until something changes", async () => {
  const view = setup()
  const input = { ...view.input, servers: ["docs"], tools: { read: tools.read!, docs_search: tools.docs_search! } }
  await view.router(input)
  await view.router({ ...input, turnID: "u2" })
  expect(view.outcomes()).toEqual(["too-few-groups"])
  await view.router(view.input)
  await view.router(input)
  expect(view.outcomes()).toEqual(["too-few-groups", "routed", "too-few-groups"])
})

test("routing times out once, and does not retry on subsequent model steps", async () => {
  const view = setup()
  view.fetcher.mockImplementation((_url, init) => new Promise((_resolve, reject) => {
    init!.signal!.addEventListener("abort", () => reject(new Error("aborted")), { once: true })
  }))
  expect(await view.router(view.input)).toBe(tools)
  expect(await view.router(view.input)).toBe(tools)
  expect(view.fetcher).toHaveBeenCalledTimes(1)
  expect(view.outcomes()).toEqual(["timeout"])
})

test("disabling routing takes effect without restart and clears cached choices", async () => {
  const view = setup()
  await view.router(view.input)
  view.enable(false)
  expect(await view.router(view.input)).toBe(tools)
  view.enable(true)
  await view.router(view.input)
  expect(view.fetcher).toHaveBeenCalledTimes(2)
})

test("ambiguous sanitized MCP prefixes are never hidden", async () => {
  const view = setup()
  const result = await view.router({ ...view.input, servers: ["docs", "github", "git.hub", "git_hub"],
    tools: { ...tools, git_hub_test: { description: "Ambiguous server" } },
  })
  expect(result).toHaveProperty("git_hub_test")
})

test("MCP names cannot hide built-in tools with a matching prefix", async () => {
  const view = setup()
  const input = { ...view.input, servers: ["docs", "apply"], tools: {
    read: tools.read!, apply_patch: { description: "Edit files" },
    docs_search: tools.docs_search!, apply_remote: { description: "Remote application API" },
  } }
  view.fetcher.mockImplementation(async () => Response.json({ answers: {
    g0: { type: "noul", noul: 0.01 }, g1: { type: "noul", noul: 0.99 },
  } }))
  const result = await view.router(input)
  expect(result).toHaveProperty("apply_patch")
  expect(result).not.toHaveProperty("apply_remote")
})

test("oversized catalogs and cancelled requests bypass routing", async () => {
  const view = setup()
  const controller = new AbortController()
  controller.abort()
  expect(await view.router({ ...view.input, abort: controller.signal })).toBe(tools)
  const many = Object.fromEntries(Array.from({ length: 500 }, (_, index) => [`docs_tool${index}`, { description: "x".repeat(512) }]))
  const large = { ...tools, ...many }
  expect(await view.router({ ...view.input, tools: large })).toBe(large)
  expect(view.fetcher).not.toHaveBeenCalled()
})

test("routing sends bounded conversational text, excluding reasoning and tool output", async () => {
  const view = setup()
  await view.router({ ...view.input, messages: [
    { role: "tool", content: "SECRET_TOOL_OUTPUT" },
    { role: "assistant", content: [{ type: "reasoning", text: "SECRET_REASONING" }, { type: "text", text: "Plan" }] },
    { role: "user", content: "x".repeat(10000) },
  ] })
  const body = view.fetcher.mock.calls[0]![1]!.body as string
  expect(body).not.toContain("SECRET_")
  expect(JSON.parse(body).state.context.at(-1).text).toHaveLength(2000)
})
