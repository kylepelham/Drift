import { afterEach, expect, test } from "bun:test"
import type { Permission } from "@opencode-ai/sdk/client"
import { createActions } from "../src/engine/actions"
import { reduce } from "../src/engine/events"
import { createEngineState, type QuestionRequest } from "../src/engine/store"
import type { DriftPermission } from "../src/state/permission-attention"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

const originalFetch = globalThis.fetch
afterEach(() => {
  globalThis.fetch = originalFetch
})

const home = "C:/work"
const other = "C:/other"

function permission(id: string, directory?: string): Permission {
  return {
    id,
    type: "bash",
    sessionID: "session",
    messageID: "message",
    title: id,
    metadata: directory ? { directory } : {},
    time: { created: 1 },
  } as Permission
}

function question(id: string, directory?: string): QuestionRequest {
  return { id, sessionID: "session", questions: [], ...(directory ? { directory } : {}) }
}

function harness() {
  const [state, set] = createEngineState()
  set("directory", home)
  const actions = createActions(
    () => ({}) as never,
    state,
    set,
    () => ({ url: "http://engine.test" }),
  )
  return { state, set, actions }
}

test("permission and question polling failures reconcile independently", async () => {
  const { state, set, actions } = harness()
  set("permissions", "session", [permission("permission", home)])
  set("questions", "session", [question("question", home)])
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const path = new URL(String(input)).pathname
    return path === "/permission" ? new Response("failed", { status: 500 }) : Response.json([])
  }) as typeof fetch

  await actions.refreshPermissions([home])
  expect(state.permissions.session?.map((item) => item.id)).toEqual(["permission"])
  expect(state.questions.session ?? []).toEqual([])

  set("questions", "session", [question("question", home)])
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const path = new URL(String(input)).pathname
    return path === "/question" ? new Response("failed", { status: 500 }) : Response.json([])
  }) as typeof fetch

  await actions.refreshPermissions([home])
  expect(state.permissions.session ?? []).toEqual([])
  expect(state.questions.session?.map((item) => item.id)).toEqual(["question"])
})

test("network and malformed polling responses preserve pending asks", async () => {
  const { state, set, actions } = harness()
  set("permissions", "session", [permission("permission", home)])
  set("questions", "session", [question("question", home)])
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    if (new URL(String(input)).pathname === "/permission") throw new Error("offline")
    return new Response("not-json", { status: 200 })
  }) as typeof fetch

  await actions.refreshPermissions([home])
  expect(state.permissions.session?.map((item) => item.id)).toEqual(["permission"])
  expect(state.questions.session?.map((item) => item.id)).toEqual(["question"])
})

test("successful scoped snapshots remove confirmed asks but preserve unknown and other directories", async () => {
  const { state, set, actions } = harness()
  set("permissions", "session", [permission("home", home), permission("unknown"), permission("other", other)])
  set("questions", "session", [question("home", home), question("unknown"), question("other", other)])
  globalThis.fetch = (async () => Response.json([])) as typeof fetch

  await actions.refreshPermissions([home])
  expect(state.permissions.session?.map((item) => item.id)).toEqual(["unknown", "other"])
  expect(state.questions.session?.map((item) => item.id)).toEqual(["unknown", "other"])
})

test("legacy polling does not remove pending v2 permissions", async () => {
  const { state, set, actions } = harness()
  reduce(
    set,
    {
      type: "permission.v2.asked",
      properties: {
        id: "v2",
        sessionID: "session",
        action: "bash",
        resources: ["git status"],
        save: ["git *"],
        metadata: {},
      },
    } as never,
    home,
  )
  const paths: string[] = []
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    paths.push(new URL(String(input)).pathname)
    return Response.json([])
  }) as typeof fetch

  await actions.refreshPermissions([home])
  expect(paths).toContain("/permission")
  const request = state.permissions.session?.[0] as DriftPermission
  expect(request.id).toBe("v2")
  expect(request.driftProtocol).toBe("v2")
  expect(request.metadata.always).toEqual(["git *"])
})

test("new SSE asks survive an older empty polling snapshot", async () => {
  const { state, set, actions } = harness()
  const pending: ((response: Response) => void)[] = []
  globalThis.fetch = (() => new Promise<Response>((resolve) => pending.push(resolve))) as typeof fetch

  const refresh = actions.refreshPermissions([home])
  reduce(
    set,
    {
      type: "permission.asked",
      properties: { id: "permission", sessionID: "session", permission: "bash", metadata: {} },
    } as never,
    home,
  )
  reduce(
    set,
    { type: "question.asked", properties: { id: "question", sessionID: "session", questions: [] } } as never,
    home,
  )
  for (const resolve of pending) resolve(Response.json([]))
  await refresh

  expect(state.permissions.session?.map((item) => item.id)).toEqual(["permission"])
  expect(state.questions.session?.map((item) => item.id)).toEqual(["question"])

  globalThis.fetch = (async () => Response.json([])) as typeof fetch
  await actions.refreshPermissions([home])
  expect(state.permissions.session ?? []).toEqual([])
  expect(state.questions.session ?? []).toEqual([])
})

test("a newer SSE reply is not resurrected by an older snapshot", async () => {
  const { state, set, actions } = harness()
  set("permissions", "session", [permission("permission", home)])
  const pending: { path: string; resolve: (response: Response) => void }[] = []
  globalThis.fetch = ((input: RequestInfo | URL) =>
    new Promise<Response>((resolve) => pending.push({ path: new URL(String(input)).pathname, resolve }))) as typeof fetch

  const refresh = actions.refreshPermissions([home])
  reduce(
    set,
    { type: "permission.replied", properties: { sessionID: "session", permissionID: "permission" } } as never,
    home,
  )
  for (const request of pending) {
    request.resolve(
      request.path === "/permission"
        ? Response.json([{ id: "permission", sessionID: "session", permission: "bash", metadata: {} }])
        : Response.json([]),
    )
  }
  await refresh

  expect(state.permissions.session ?? []).toEqual([])
})

test("overlapping polling sweeps coalesce active directories and drain new ones", async () => {
  const { actions } = harness()
  let releaseHome!: () => void
  const homePending = new Promise<void>((resolve) => (releaseHome = resolve))
  const requests: string[] = []
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const url = new URL(String(input))
    const directory = url.searchParams.get("directory") ?? ""
    requests.push(`${directory}:${url.pathname}`)
    if (directory === home) await homePending
    return Response.json([])
  }) as typeof fetch

  const first = actions.refreshPermissions([home])
  const second = actions.refreshPermissions([home, other])
  expect(first).toBe(second)
  releaseHome()
  await Promise.all([first, second])

  expect(requests.filter((request) => request.startsWith(`${home}:`))).toHaveLength(2)
  expect(requests.filter((request) => request.startsWith(`${other}:`))).toHaveLength(2)
})

test.each(["question.asked", "question.v2.asked"])("%s preserves async and blocking request metadata", (type) => {
  const { state, set } = harness()
  for (const async of [true, false, undefined]) {
    const request: QuestionRequest = {
      id: `sse-${async}`,
      sessionID: "owner",
      ...(async === undefined ? {} : { async }),
      questions: [{ header: "Scope", question: "What should change?", options: [{ label: "Tests", description: "Tests only" }] }],
      tool: { messageID: "message", callID: "call" },
    }
    reduce(set, { type, properties: request } as never, other)
    expect(state.questions.owner?.find((item) => item.id === request.id)).toEqual({ ...request, directory: other })
  }
  expect(state.questions.owner).toHaveLength(3)
})

test("polling recovers async metadata and refreshes SSE requests in their owning directory", async () => {
  const { state, set, actions } = harness()
  const requests: QuestionRequest[] = [true, false, undefined].map((async) => ({
    id: `poll-${async}`,
    sessionID: "owner",
    ...(async === undefined ? {} : { async }),
    questions: [{ header: "Scope", question: "What should change?", options: [], custom: true }],
    tool: { messageID: "message", callID: `call-${async}` },
  }))
  reduce(set, { type: "question.asked", properties: requests[0] } as never, other)
  set("questions", "session", [question("leave-home", home)])
  const polled: string[] = []
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const url = new URL(String(input))
    polled.push(`${url.pathname}:${url.searchParams.get("directory")}`)
    return Response.json(url.pathname === "/question" ? requests : [])
  }) as typeof fetch

  await actions.refreshPermissions([other])
  expect(polled.sort()).toEqual([`/permission:${other}`, `/question:${other}`])
  expect(state.questions.owner).toEqual(requests.map((request) => ({ ...request, directory: other })))
  expect(state.questions.session).toEqual([question("leave-home", home)])
})

test.each(["http", "network"])("late async reply survives %s failure and retries only the owning request", async (failure) => {
  const { clearQuestionDraft, questionDraftState, setQuestionDraftStep, updateQuestionDraft } = await import(
    "../src/state/question-drafts"
  )
  const { applyMirroredSession, selectedSession } = await import("../src/state/selection")
  const previousSelection = selectedSession()
  const { state, set, actions } = harness()
  const id = `late-${failure}`
  const sibling = `${id}-sibling`
  const current = `${id}-current`
  const drafts = [
    { selected: ["Tests"], custom: "", customSelected: false },
    { selected: [], custom: "Keep custom answer", customSelected: true },
  ]
  try {
    applyMirroredSession("current")
    set("directory", other)
    set("questions", "owner", [
      {
        ...question(id, home), sessionID: "owner", async: true,
        questions: [
          { header: "Scope", question: "What should change?", options: [{ label: "Tests", description: "Tests only" }] },
          { header: "Details", question: "Anything else?", options: [], custom: true },
        ],
      },
      { ...question(sibling, home), sessionID: "owner", async: true },
    ])
    set("questions", "current", [{ ...question(current, other), sessionID: "current", async: true }])
    drafts.forEach((draft, index) => updateQuestionDraft(id, 2, index, draft))
    setQuestionDraftStep(id, 2, 1)
    updateQuestionDraft(sibling, 1, 0, drafts[0])
    updateQuestionDraft(current, 1, 0, drafts[1])
    const before = JSON.parse(JSON.stringify(state.questions))
    const sent: { path: string; directory: string | null; method: string | undefined; body: unknown }[] = []
    let succeed = false
    globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input))
      sent.push({ path: url.pathname, directory: url.searchParams.get("directory"), method: init?.method, body: JSON.parse(String(init?.body)) })
      if (succeed) return Response.json(true)
      if (failure === "network") throw new TypeError("offline")
      return new Response("save failed", { status: 500 })
    }) as typeof fetch
    const answers = [["Tests"], ["Keep custom answer"]]

    expect(await actions.answerQuestion("owner", id, answers)).toBeFalse()
    expect(state.questions).toEqual(before)
    expect(questionDraftState(id, 2)).toEqual({ step: 1, drafts })
    expect(questionDraftState(sibling, 1).drafts[0]).toEqual(drafts[0])
    expect(questionDraftState(current, 1).drafts[0]).toEqual(drafts[1])

    succeed = true
    expect(await actions.answerQuestion("owner", id, answers)).toBeTrue()
    expect(sent).toEqual(Array.from({ length: 2 }, () => ({
      path: `/question/${id}/reply`, directory: home, method: "POST", body: { answers },
    })))
    expect(state.questions.owner?.map((item) => item.id)).toEqual([sibling])
    expect(state.questions.current).toEqual(before.current)
    expect(selectedSession()).toBe("current")
    expect(state.directory).toBe(other)

    // The reply event can arrive after the HTTP response and the card has unmounted.
    reduce(set, { type: "question.replied", properties: { sessionID: "owner", requestID: id } } as never, home)
    expect(questionDraftState(id, 2)).toEqual({
      step: 0, drafts: Array.from({ length: 2 }, () => ({ selected: [], custom: "", customSelected: false })),
    })
    expect(state.questions.owner?.map((item) => item.id)).toEqual([sibling])
    expect(questionDraftState(sibling, 1).drafts[0]).toEqual(drafts[0])
    expect(questionDraftState(current, 1).drafts[0]).toEqual(drafts[1])
  } finally {
    applyMirroredSession(previousSelection)
    for (const requestID of [id, sibling, current]) clearQuestionDraft(requestID)
  }
})

test("an async answer during polling is not resurrected by the stale question snapshot", async () => {
  const { state, set, actions } = harness()
  const request = { ...question("async-race", home), async: true }
  const sibling = { ...question("async-race-sibling", home), async: true }
  set("questions", "session", [request, sibling])
  const pending: { path: string; resolve: (response: Response) => void }[] = []
  globalThis.fetch = ((input: RequestInfo | URL, init?: RequestInit) => {
    if (init?.method === "POST") return Promise.resolve(Response.json(true))
    return new Promise<Response>((resolve) => pending.push({ path: new URL(String(input)).pathname, resolve }))
  }) as typeof fetch

  const refresh = actions.refreshPermissions([home])
  expect(await actions.answerQuestion("session", request.id, [["Tests"]])).toBeTrue()
  for (const poll of pending) poll.resolve(Response.json(poll.path === "/question" ? [request, sibling] : []))
  await refresh
  expect(state.questions.session).toEqual([sibling])

  globalThis.fetch = (async (input: RequestInfo | URL) =>
    Response.json(new URL(String(input)).pathname === "/question" ? [request, sibling] : [])) as typeof fetch
  await actions.refreshPermissions([home])
  expect(state.questions.session).toEqual([sibling])
})
