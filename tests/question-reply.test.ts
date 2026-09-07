import { afterEach, expect, test } from "bun:test"
import type { Session } from "@opencode-ai/sdk/client"
import { createActions } from "../src/engine/actions"
import { askRevision, createEngineState, type QuestionRequest } from "../src/engine/store"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

const originalFetch = globalThis.fetch
const originalSetTimeout = globalThis.setTimeout
const originalClearTimeout = globalThis.clearTimeout
afterEach(() => {
  globalThis.fetch = originalFetch
  globalThis.setTimeout = originalSetTimeout
  globalThis.clearTimeout = originalClearTimeout
})

const home = "C:/one"
const elsewhere = "C:/two"

function question(id = "q1", sessionID = "s1", directory = elsewhere): QuestionRequest {
  return { id, sessionID, directory, questions: [] }
}

function harness(reply: (request: Request) => Promise<Response>) {
  const [state, set] = createEngineState()
  set("directory", home)
  set("questions", "s1", [question(), question("sibling")])
  set("questions", "s2", [question("q1", "s2", home)])
  const requests: Request[] = []
  globalThis.fetch = ((input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init)
    requests.push(request)
    return reply(request)
  }) as typeof fetch
  const actions = createActions(
    () => ({}) as never,
    state,
    set,
    () => ({ url: "http://engine.test", headers: { authorization: "Bearer test-only" } }),
  )
  return { state, set, actions, requests }
}

function deferredResponse() {
  let resolve!: (response: Response) => void
  const promise = new Promise<Response>((done) => (resolve = done))
  return { promise, resolve }
}

// Advance only the reply deadline, without sleeps or a race against the test runner's clock.
function replyClock() {
  const timers = new Map<number, { fire: () => void; delay: number | undefined }>()
  let next = 0
  globalThis.setTimeout = ((handler: () => void, delay?: number) => {
    const id = ++next
    timers.set(id, { fire: handler, delay })
    return id
  }) as typeof setTimeout
  globalThis.clearTimeout = ((id: number) => {
    timers.delete(id)
  }) as typeof clearTimeout
  return timers
}

test.each([{ answers: [["Tests"], ["Keep custom answer"]] }, { answers: [] }, { answers: null }])(
  "identical question payloads share one request: %j",
  async ({ answers }) => {
    const pending = deferredResponse()
    const { state, actions, requests } = harness(() => pending.promise)
    const timers = replyClock()
    const first = actions.answerQuestion("s1", "q1", answers)
    const second = actions.answerQuestion("s1", "q1", answers?.map((row) => [...row]) ?? null)

    expect(first).toBe(second)
    expect(requests).toHaveLength(1)
    expect(timers.size).toBe(1)
    expect(new URL(requests[0].url).pathname).toBe(`/question/q1/${answers ? "reply" : "reject"}`)
    expect(await requests[0].json()).toEqual(answers ? { answers } : {})
    expect(state.questions.s1).toHaveLength(2)
    pending.resolve(new Response(null, { status: 204 }))
    expect(await Promise.all([first, second])).toEqual([true, true])
    expect(state.questions.s1).toEqual([question("sibling")])
    expect(state.questions.s2).toEqual([question("q1", "s2", home)])
    expect(timers.size).toBe(0)
  },
)

test.each([
  { first: [["Tests"]], conflict: [["Code"]] },
  { first: [["Tests"]], conflict: null },
  { first: null, conflict: [["Tests"]] },
])("conflicting question payloads are not sent or reported accepted: %j", async ({ first, conflict }) => {
  const pending = deferredResponse()
  const { actions, requests } = harness(() => pending.promise)
  const accepted = actions.answerQuestion("s1", "q1", first)
  expect(await actions.answerQuestion("s1", "q1", conflict)).toBe(false)
  expect(requests).toHaveLength(1)
  pending.resolve(new Response(null, { status: 200 }))
  expect(await accepted).toBe(true)
})

test("payload comparison uses the sent snapshot, not a mutable answers array", async () => {
  const pending = deferredResponse()
  const { actions, requests } = harness(() => pending.promise)
  const answers = [["Tests"]]
  const first = actions.answerQuestion("s1", "q1", answers)
  answers[0][0] = "Code"
  expect(await actions.answerQuestion("s1", "q1", answers)).toBe(false)
  expect(actions.answerQuestion("s1", "q1", [["Tests"]])).toBe(first)
  expect(requests).toHaveLength(1)
  expect(await requests[0].json()).toEqual({ answers: [["Tests"]] })
  pending.resolve(new Response(null, { status: 200 }))
  expect(await first).toBe(true)
})

test.each(["http", "network"])("%s failures retain pending questions and release the flight for retry", async (failure) => {
  let succeed = false
  const { state, actions, requests } = harness(async () => {
    if (succeed) return new Response(null, { status: 200 })
    if (failure === "network") throw new TypeError("offline")
    return new Response("failed", { status: 500 })
  })
  const timers = replyClock()
  const before = JSON.parse(JSON.stringify(state.questions))
  const first = actions.answerQuestion("s1", "q1", [["Tests"]])
  const duplicate = actions.answerQuestion("s1", "q1", [["Tests"]])
  expect(await Promise.all([first, duplicate])).toEqual([false, false])
  expect(requests).toHaveLength(1)
  expect(state.questions).toEqual(before)
  expect(askRevision(state, "question", elsewhere)).toBe(0)
  expect(timers.size).toBe(0)

  succeed = true
  expect(await actions.answerQuestion("s1", "q1", [["Tests"]])).toBe(true)
  expect(requests).toHaveLength(2)
  expect(state.questions.s1).toEqual([question("sibling")])
  expect(state.questions.s2).toEqual(before.s2)
  expect(timers.size).toBe(0)
})

test.each([{ answers: [["Tests"]] }, { answers: null }])(
  "reply timeout aborts transport but retains the unconfirmed card: %j",
  async ({ answers }) => {
    let succeed = false
    const { state, actions, requests } = harness((request) => {
      if (succeed) return Promise.resolve(new Response(null, { status: 200 }))
      return new Promise<Response>((_resolve, reject) => {
        request.signal.addEventListener("abort", () => reject(request.signal.reason), { once: true })
      })
    })
    const timers = replyClock()
    const before = JSON.parse(JSON.stringify(state.questions))
    const first = actions.answerQuestion("s1", "q1", answers)
    const duplicate = actions.answerQuestion("s1", "q1", answers)
    expect(timers.size).toBe(1)
    const timer = [...timers.values()][0]
    expect(timer.delay).toBe(8000)
    expect(requests[0].signal.aborted).toBe(false)
    timer.fire()
    expect(await Promise.all([first, duplicate])).toEqual([false, false])
    expect(requests).toHaveLength(1)
    expect(requests[0].signal.aborted).toBe(true)
    expect(state.questions).toEqual(before)
    expect(askRevision(state, "question", elsewhere)).toBe(0)
    expect(state.notices).toHaveLength(0)
    expect(timers.size).toBe(0)

    succeed = true
    expect(await actions.answerQuestion("s1", "q1", answers)).toBe(true)
    expect(requests).toHaveLength(2)
    expect(requests[1].signal.aborted).toBe(false)
    expect(state.questions.s1).toEqual([question("sibling")])
    expect(timers.size).toBe(0)
  },
)

test("a successful reply keeps its owning workspace when the active workspace changes", async () => {
  const pending = deferredResponse()
  const { state, set, actions, requests } = harness(() => pending.promise)
  const reply = actions.answerQuestion("s1", "q1", [["Tests"]])
  const request = requests[0]
  expect(new URL(request.url).searchParams.get("directory")).toBe(elsewhere)
  expect(request.method).toBe("POST")
  expect(request.headers.get("authorization")).toBe("Bearer test-only")
  set("directory", "C:/three")
  set("questions", "s1", [question(), question("sibling"), question("q1", "s1", home)])
  pending.resolve(new Response(null, { status: 200 }))

  expect(await reply).toBe(true)
  expect(state.questions.s1).toEqual([question("sibling"), question("q1", "s1", home)])
  expect(state.questions.s2).toEqual([question("q1", "s2", home)])
  expect(state.directory).toBe("C:/three")
  expect(askRevision(state, "question", elsewhere)).toBe(1)
  expect(askRevision(state, "question", home)).toBe(0)
})

test("questions without a directory use the owning session before the active workspace", async () => {
  const { state, set, actions, requests } = harness(async () => new Response(null, { status: 200 }))
  set("sessions", "s1", { id: "s1", directory: elsewhere } as Session)
  set("questions", "s1", [{ ...question(), directory: undefined }])
  expect(await actions.answerQuestion("s1", "q1", null)).toBe(true)
  expect(new URL(requests[0].url).searchParams.get("directory")).toBe(elsewhere)
  expect(state.questions.s1).toEqual([])
})

test("single-flight is scoped to both session and question IDs", async () => {
  const pending = deferredResponse()
  const { state, actions, requests } = harness(() => pending.promise)
  const replies = [
    actions.answerQuestion("s1", "q1", [["Tests"]]),
    actions.answerQuestion("s1", "sibling", null),
    actions.answerQuestion("s2", "q1", [["Code"]]),
  ]
  expect(requests).toHaveLength(3)
  pending.resolve(new Response(null, { status: 200 }))
  expect(await Promise.all(replies)).toEqual([true, true, true])
  expect(state.questions.s1).toEqual([])
  expect(state.questions.s2).toEqual([])
})
