import { afterEach, expect, test } from "bun:test"
import { createActions } from "../src/engine/actions"
import type { Client } from "../src/engine/native/client"
import { EngineError } from "../src/engine/native/client"
import type { components } from "../src/engine/native/types"
import { createEngineState } from "../src/engine/store"
import { rememberProviderCatalog } from "../src/state/provider-cache"

type Session = components["schemas"]["Session"]

// refreshProviders remembers the catalog in a module signal; other suites expect it empty.
afterEach(() => rememberProviderCatalog([], [], {}))

function session(id: string, workspaceId = "w1"): Session {
  return { id, workspaceId, visibility: "sibling", title: id, agent: "build", createdAt: 1, updatedAt: 2 }
}

function harness(overrides: Partial<Client> = {}) {
  const calls: { method: string; args: unknown[] }[] = []
  const record =
    <T>(method: string, result: T) =>
    (...args: unknown[]) => {
      calls.push({ method, args })
      return Promise.resolve(result)
    }
  const client = {
    sessions: record("sessions", [session("ses_1"), session("ses_2")]),
    createSession: record("createSession", session("ses_new")),
    updateSession: record("updateSession", { ...session("ses_1"), title: "Renamed" }),
    messages: record("messages", []),
    submit: record("submit", { session: session("ses_1"), message: {} }),
    abort: record("abort", { aborted: true }),
    providers: record("providers", [
      { id: "anthropic", name: "Anthropic", connected: true, credential: "keychain", models: { claude: { id: "claude", name: "Claude", reasoning: true, attachment: true, temperature: true, family: "c", release_date: "", limit: { context: 200000, output: 8192 }, cost: { input: 3, output: 15, cache_read: 0.3, cache_write: 3.75 }, profile: "edit" } } },
      { id: "openai", name: "OpenAI", connected: false, models: {} },
    ]),
    permissions: record("permissions", []),
    replyPermission: record("replyPermission", undefined),
    setProviderKey: record("setProviderKey", undefined),
    ...overrides,
  } as unknown as Client
  const [state, set] = createEngineState()
  set("directory", "C:/repo")
  const workspaces = () => ({ path: (id: string) => (id === "w1" ? "C:/repo" : undefined), id: (path: string) => (path === "C:/repo" ? "w1" : undefined) })
  const actions = createActions(() => client, state, set, workspaces)
  return { actions, state, calls }
}

test("loading sessions for a directory scopes the request to its workspace", async () => {
  const h = harness()
  await h.actions.loadSessions("C:/repo")
  expect(h.calls[0]).toEqual({ method: "sessions", args: [{ workspace: "w1", limit: 200 }] })
  expect(Object.keys(h.state.sessions).sort()).toEqual(["ses_1", "ses_2"])
  expect(h.state.sessions.ses_1!.directory).toBe("C:/repo")
})

test("send maps model, files and reasoning effort onto the native prompt", async () => {
  const h = harness()
  const result = await h.actions.send("ses_1", "hello", {
    model: { providerID: "anthropic", modelID: "claude" },
    agent: "build",
    variant: "high",
    files: [{ mime: "image/png", url: "data:image/png;base64,AAAA", filename: "shot.png" }],
  })
  expect(result).toEqual({ ok: true })
  const submitted = h.calls[0] as { method: string; args: [string, { submissionId?: string }] }
  expect(typeof submitted.args[1].submissionId).toBe("string")
  delete submitted.args[1].submissionId
  expect(h.calls[0]).toEqual({
    method: "submit",
    args: [
      "ses_1",
      {
        parts: [
          { type: "text", text: "hello" },
          { type: "file", mime: "image/png", name: "shot.png", url: "data:image/png;base64,AAAA" },
        ],
        model: { provider: "anthropic", model: "claude" },
        thinkingBudget: 20_000,
      },
    ],
  })
})

test("send failures land in the session's error slot", async () => {
  const h = harness({ submit: () => Promise.reject(new EngineError(409, "/turns", "busy", "session is already running a turn")) })
  const result = await h.actions.send("ses_1", "again", { model: null, agent: "build" })
  expect(result).toEqual({ ok: false, error: "Prompt failed: session is already running a turn" })
  expect(h.state.errors.ses_1).toBe("Prompt failed: session is already running a turn")
  expect(await h.actions.send("ses_1", "   ", { model: null, agent: "build" })).toEqual({ ok: false, error: "Prompt failed: the prompt is empty" })
})

test("providers become the catalog shape the picker reads", async () => {
  const h = harness()
  await h.actions.refreshProviders()
  expect(h.state.connected).toEqual(["anthropic"])
  expect(h.state.providers.map((p) => p.id)).toEqual(["anthropic", "openai"])
  const model = h.state.providers[0]!.models.claude!
  expect(model.capabilities.toolcall).toBe(true)
  expect(model.cost.cache).toEqual({ read: 0.3, write: 3.75 })
  expect((model as { limit: { context: number } }).limit.context).toBe(200000)
})

test("permission replies translate reject to deny and forget stale requests", async () => {
  const replies: unknown[][] = []
  const h = harness({
    replyPermission: (id: string, body: unknown) => {
      replies.push([id, body])
      return id === "gone" ? Promise.reject(new EngineError(404, "/p")) : Promise.resolve()
    },
  })
  await h.actions.replyPermission("ses_1", "perm_1", "reject")
  expect(replies[0]).toEqual(["perm_1", { reply: "deny" }])
  h.state.permissions.ses_1 = [{ id: "gone", type: "bash", sessionID: "ses_1", messageID: "m", title: "t", metadata: {}, time: { created: 0 } }]
  await h.actions.replyPermission("ses_1", "gone", "once")
  expect(h.state.permissions.ses_1).toEqual([])
})

test("new sessions are created in the active workspace and removal archives", async () => {
  const h = harness()
  const created = await h.actions.newSession()
  expect(h.calls[0]).toEqual({ method: "createSession", args: [{ workspaceId: "w1" }] })
  expect(created?.id).toBe("ses_new")
  expect(h.state.sessions.ses_new).toBeDefined()
  expect(h.state.loaded.ses_new).toBe(true)
  expect(h.state.transcripts.ses_new).toEqual([])
  await h.actions.remove("ses_new")
  expect(h.calls.at(-1)).toEqual({ method: "updateSession", args: ["ses_new", { archived: true }] })
  expect(h.state.sessions.ses_new).toBeUndefined()
})

test("session listings page to the end and restore running status from every page", async () => {
  const pages: unknown[][] = []
  const full = Array.from({ length: 200 }, (_, i) => ({ ...session(`ses_${i}`), running: i === 7 }))
  const h = harness({
    sessions: ((params: { before?: string }) => {
      pages.push([params])
      const page = params.before ? [{ ...session("ses_tail"), running: true }] : full
      return Promise.resolve(page)
    }) as unknown as Client["sessions"],
  })
  await h.actions.loadSessions("C:/repo")
  expect(pages.length).toBe(2)
  expect(pages[1]).toEqual([{ workspace: "w1", before: "ses_199", limit: 200 }])
  expect(Object.keys(h.state.sessions).length).toBe(201)
  expect(h.state.status.ses_tail).toEqual({ type: "busy" })
  expect(h.state.status.ses_7).toEqual({ type: "busy" })
  expect(h.state.status.ses_0).toEqual({ type: "idle" })
})

test("purge deletes for real and only reports success when the engine confirms", async () => {
  const deleted: string[] = []
  const ok = harness({ deleteSession: (id: string) => (deleted.push(id), Promise.resolve()) })
  ok.state.sessions.ses_1 = { id: "ses_1", directory: "C:/repo" } as never
  expect(await ok.actions.purgeSession("ses_1")).toBe(true)
  expect(deleted).toEqual(["ses_1"])
  expect(ok.state.sessions.ses_1).toBeUndefined()
  expect(ok.calls.some((c) => c.method === "updateSession")).toBe(false)
  const failing = harness({ deleteSession: () => Promise.reject(new EngineError(500, "/sessions/x", "store")) })
  failing.state.sessions.ses_1 = { id: "ses_1", directory: "C:/repo" } as never
  expect(await failing.actions.purgeSession("ses_1")).toBe(false)
  expect(failing.state.sessions.ses_1).toBeDefined()
  const gone = harness({ deleteSession: () => Promise.reject(new EngineError(404, "/sessions/x")) })
  expect(await gone.actions.purgeSession("ses_1")).toBe(true)
})

test("every send carries a fresh submission id", async () => {
  const h = harness()
  await h.actions.send("ses_1", "a", { model: null, agent: "build" })
  await h.actions.send("ses_1", "b", { model: null, agent: "build" })
  const ids = h.calls.filter((c) => c.method === "submit").map((c) => (c.args[1] as { submissionId: string }).submissionId)
  expect(ids).toHaveLength(2)
  expect(ids[0]).toBeTruthy()
  expect(ids[0]).not.toBe(ids[1])
})

test("hydration rejects when any of its loads fail, instead of pretending the snapshot landed", async () => {
  const { hydrateFrom } = await import("../src/engine/index")
  const good = { refreshProviders: async () => true, loadSessions: async () => undefined, refreshPermissions: async () => undefined, refreshAgents: async () => undefined }
  await hydrateFrom(good, "C:/repo")
  await expect(hydrateFrom({ ...good, loadSessions: () => Promise.reject(new Error("db")) }, "C:/repo")).rejects.toThrow("db")
  await expect(hydrateFrom({ ...good, refreshProviders: async () => false }, null)).rejects.toThrow("provider catalog")
  await expect(hydrateFrom({ ...good, refreshPermissions: () => Promise.reject(new Error("perm")) }, null)).rejects.toThrow("perm")
})

test("action agents are listed for Settings but hidden from the composer, with their pins and prompts", async () => {
  const config = {
    agents: [
      { name: "build", description: "", builtin: true, kind: "primary" },
      { name: "title", description: "", builtin: true, kind: "action", prompt: "Name it.", model: { provider: "openai", model: "gpt-5-nano" } },
      { name: "explore", description: "", builtin: true, kind: "subagent", prompt: "Search." },
    ],
    commands: [],
    skills: [],
  }
  const h = harness({ workspaceConfig: () => Promise.resolve(config) } as Partial<Client>)
  await h.actions.refreshAgents()
  const [build, title, explore] = h.state.agents as ((typeof h.state.agents)[number] & { hidden?: boolean; prompt?: string })[]
  expect(build!.hidden).toBeFalse()
  expect(build!.mode).toBe("primary")
  expect(explore!.mode).toBe("subagent")
  expect(explore!.prompt).toBe("Search.")
  expect(title!.hidden).toBeTrue()
  expect(title!.prompt).toBe("Name it.")
  expect(title!.model).toEqual({ providerID: "openai", modelID: "gpt-5-nano" })
})

test("/compact asks the engine to compact and reports a refusal; the auto setting round-trips", async () => {
  const h = harness({
    compactSession: (id: string) => (id === "ses_busy" ? Promise.reject(new EngineError(409, "/sessions/ses_busy/compact", "busy", "a turn is running")) : Promise.resolve(undefined)),
    putSettings: (body: { autoCompact: boolean }) => Promise.resolve(body),
  } as Partial<Client>)
  await h.actions.summarize("ses_1")
  expect(h.state.notices.length).toBe(0)
  await h.actions.summarize("ses_busy")
  expect(h.state.notices.some((n) => n.title === "Couldn't compact" && n.message === "a turn is running")).toBeTrue()
  expect(await h.actions.setAutoCompact(false)).toEqual({ autoCompact: false })
})

test("undo and redo apply the engine's session and report refusals", async () => {
  const h = harness({
    revertSession: (id: string, messageId: string) =>
      id === "ses_busy"
        ? Promise.reject(new EngineError(409, `/sessions/${id}/revert`, "busy", "stop the running turn first"))
        : Promise.resolve({ session: { ...session(id), revert: { messageId } }, kept: [] }),
    unrevertSession: (id: string) => Promise.resolve({ session: session(id), kept: ["src/app.ts"] }),
  } as Partial<Client>)
  expect(await h.actions.revert("ses_1", "msg_2")).toBeTrue()
  expect((h.state.sessions.ses_1 as { revert?: { messageID: string } }).revert?.messageID).toBe("msg_2")
  expect(h.state.notices.length).toBe(0)
  expect(await h.actions.unrevert("ses_1")).toBeTrue()
  expect((h.state.sessions.ses_1 as { revert?: unknown }).revert).toBeUndefined()
  expect(h.state.notices.some((n) => n.title === "Kept your changes" && n.message.includes("src/app.ts"))).toBeTrue()
  expect(await h.actions.revert("ses_busy", "msg_2")).toBeFalse()
  expect(h.state.notices.some((n) => n.title === "Couldn't undo")).toBeTrue()
})

test("switching a retrying turn's model sends the native model ref and reports refusals", async () => {
  const sent: unknown[] = []
  const h = harness({
    switchRetryModel: (id: string, model: unknown) => {
      sent.push([id, model])
      return id === "ses_idle" ? Promise.reject(new EngineError(409, "/sessions/ses_idle/retry", "not_retrying", "the session is not waiting to retry")) : Promise.resolve(undefined)
    },
  } as Partial<Client>)
  expect(await h.actions.switchRetryModel("ses_1", "msg_1", { providerID: "openai", modelID: "gpt-5" })).toEqual({ ok: true })
  expect(sent[0]).toEqual(["ses_1", { provider: "openai", model: "gpt-5" }])
  expect(await h.actions.switchRetryModel("ses_idle", "msg_1", { providerID: "openai", modelID: "gpt-5" })).toEqual({ ok: false, error: "the session is not waiting to retry" })
})

test("a reviewed branch becomes a top-level session linked to its source", async () => {
  const draft = { goal: "fix lint", title: "Fix lint", summary: "Parser tidied.", excerpts: "", cutoff: "msg_9" }
  const h = harness({
    draftBranch: (id: string, goal: string) => Promise.resolve({ ...draft, goal: `${goal} (${id})` }),
    createBranch: (_id: string, body: typeof draft) =>
      Promise.resolve({ ...session("ses_branch"), parentId: "ses_1", branchCutoff: body.cutoff, title: body.title }),
  } as Partial<Client>)
  expect((await h.actions.draftBranch("ses_1", "fix lint"))?.goal).toBe("fix lint (ses_1)")
  const created = await h.actions.branch("ses_1", draft)
  expect(created?.id).toBe("ses_branch")
  expect(h.state.sessions.ses_branch!.parentID).toBeUndefined()
  expect(h.state.links.ses_branch).toBe("ses_1")
  expect(h.state.loaded.ses_branch).toBeTrue()
})

test("a failed draft reports a notice instead of throwing", async () => {
  const h = harness({ draftBranch: () => Promise.reject(new EngineError(502, "/sessions/ses_1/branch/draft", "draft", "the model returned no handoff")) } as Partial<Client>)
  expect(await h.actions.draftBranch("ses_1", "fix lint")).toBeUndefined()
  expect(h.state.notices.some((n) => n.title === "Couldn't draft the branch")).toBeTrue()
})

test("fork opens the copy as a new top-level session", async () => {
  const h = harness({ forkSession: (id: string) => Promise.resolve({ ...session("ses_fork"), title: `${id} (fork)` }) } as Partial<Client>)
  const fork = await h.actions.fork("ses_1", "active")
  expect(fork?.id).toBe("ses_fork")
  expect(h.state.sessions.ses_fork!.title).toBe("ses_1 (fork)")
  expect(h.state.sessions.ses_fork!.parentID).toBeUndefined()
})

test("moving resolves the destination workspace and reports the engine's busy refusal", async () => {
  const moved = harness({ moveSession: (_id: string, workspaceId: string) => Promise.resolve({ moved: ["ses_1", workspaceId] }) } as Partial<Client>)
  expect(await moved.actions.moveSession("ses_1", "C:/repo")).toEqual({ ok: true, moved: ["ses_1", "w1"] })
  expect((await moved.actions.moveSession("ses_1", "D:/unknown")).ok).toBeFalse()
  const busy = harness({ moveSession: () => Promise.reject(new EngineError(409, "/sessions/ses_1/move", "busy", "stop the running turn first")) } as Partial<Client>)
  expect(await busy.actions.moveSession("ses_1", "C:/repo")).toEqual({ ok: false, moved: [], error: "stop the running turn first" })
})

test("re-pointing a workspace folder moves nothing but waits for running threads", async () => {
  const idle = harness()
  expect(await idle.actions.moveWorkspaceSessions("C:/repo", "D:/repo")).toEqual({ ok: true, moved: [] })
  const running = harness({ sessions: () => Promise.resolve([{ ...session("ses_1"), running: true }]) } as Partial<Client>)
  expect((await running.actions.moveWorkspaceSessions("C:/repo", "D:/repo")).ok).toBeFalse()
})
