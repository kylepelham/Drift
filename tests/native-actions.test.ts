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
