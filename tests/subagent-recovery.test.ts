import { afterEach, expect, test } from "bun:test"
import { classifyRecoverableError } from "../src/engine/error"
import { createEngineState } from "../src/engine/store"
import {
  clearRecoverableInterruption,
  mergeInterruptions,
  recordRecoverableInterruption,
  recoverableForSession,
  recoveryNavigationTarget,
} from "../src/state/recovery"
import type { DriftStore, RecoverableInterruption } from "../src/state/store"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

const saved: RecoverableInterruption[] = []
const store = {
  interruptions: async () => saved,
  saveInterruption: async (item: RecoverableInterruption) => {
    const index = saved.findIndex((current) => current.sessionId === item.sessionId && current.identity === item.identity)
    if (index < 0) saved.push(item)
    else saved[index] = { ...item, dismissedAt: saved[index].dismissedAt }
  },
  dismissInterruption: async () => undefined,
  clearInterruptions: async (sessionId: string) => {
    for (let index = saved.length - 1; index >= 0; index--) if (saved[index].sessionId === sessionId) saved.splice(index, 1)
  },
} as DriftStore

afterEach(async () => {
  clearRecoverableInterruption("child", false, store)
  clearRecoverableInterruption("other", false, store)
  await Bun.sleep(0)
  saved.splice(0)
})

test("classifier separates recoverable model failures from cancellation and terminal input failures", () => {
  const classify = (name: string, data: Record<string, unknown>) => classifyRecoverableError({ name, data })?.kind
  expect(classify("APIError", { message: "5 hour usage limit reached", isRetryable: false })).toBe("usage")
  expect(classify("APIError", { message: "Too many requests", statusCode: 429, isRetryable: true })).toBe("rate_limit")
  expect(classify("APIError", { message: "Model is overloaded", statusCode: 503, isRetryable: true })).toBe("unavailable")
  expect(classify("ProviderAuthError", { message: "invalid key", providerID: "openai" })).toBe("provider_auth")
  expect(classify("APIError", { message: "socket hang up", isRetryable: true })).toBe("transient")
  expect(classifyRecoverableError({ name: "MessageAbortedError", data: { message: "aborted" } })).toBeNull()
  expect(classifyRecoverableError({ name: "AbortError", data: { message: "cancelled by user" } })).toBeNull()
  expect(classifyRecoverableError({ name: "ContextOverflowError", data: { message: "context length exceeded" } })).toBeNull()
  expect(classifyRecoverableError({ name: "ContentFilterError", data: { message: "blocked" } })).toBeNull()
})

function interruption(overrides: Partial<RecoverableInterruption> = {}): RecoverableInterruption {
  return {
    sessionId: "child",
    identity: "message-1",
    workspaceId: "workspace-b",
    directory: "D:/work/beta",
    threadTitle: "Research API",
    parentSessionId: "parent",
    providerId: "anthropic",
    modelId: "claude-sonnet",
    kind: "usage",
    reason: "usage limit reached",
    errorName: "APIError",
    createdAt: 1,
    updatedAt: 1,
    ...overrides,
  }
}

test("interruption merge deduplicates by session and identity across restart hydration", () => {
  const older = interruption()
  const current = interruption({ reason: "new reason", updatedAt: 2 })
  const other = interruption({ sessionId: "other", identity: "message-2" })
  expect(mergeInterruptions([older], [current, other])).toEqual([current, other])
})

test("background recovery notifications exclude the selected and dismissed sessions", async () => {
  const { backgroundRecoveries } = await import("../src/ui/notifications")
  const entries = [interruption(), interruption({ sessionId: "other", identity: "message-2" })]
  expect(backgroundRecoveries(entries, "child").map((item) => item.sessionId)).toEqual(["other"])
  expect(backgroundRecoveries([{ ...entries[1], dismissedAt: 3 }], null)).toEqual([])
})

test("recovery navigation targets the exact owning workspace and existing child", () => {
  expect(
    recoveryNavigationTarget(interruption(), [
      { id: "workspace-a", path: "C:/work/alpha", name: "A", icon: "", lastUsed: 0 },
      { id: "workspace-b", path: "d:\\WORK\\beta\\", name: "B", icon: "", lastUsed: 0 },
    ]),
  ).toEqual({ workspaceId: "workspace-b", sessionId: "child" })
})

test("recovery prompts the same session with the explicitly selected model and durable-state instruction", async () => {
  const { createActions, RECOVERY_INSTRUCTION } = await import("../src/engine/actions")
  const [state, set] = createEngineState()
  let request: unknown
  const client = {
    session: {
      promptAsync: async (input: unknown) => {
        request = input
        return { data: {} }
      },
    },
  }
  const actions = createActions(() => client as never, state, set, () => undefined)
  expect(
    await actions.recover("child", {
      model: { providerID: "openai", modelID: "gpt-5" },
      agent: "explore",
    }),
  ).toEqual({ ok: true })
  expect(request).toMatchObject({
    path: { id: "child" },
    body: {
      model: { providerID: "openai", modelID: "gpt-5" },
      agent: "explore",
      parts: [{ type: "text", text: RECOVERY_INSTRUCTION, metadata: { generated: true } }],
    },
  })
  expect(RECOVERY_INSTRUCTION).toContain("latest durable state")
  expect(RECOVERY_INSTRUCTION).toContain("Do not blindly repeat tools")
})

test("recovery targets the interrupted session directory instead of the active workspace client", async () => {
  const { createActions } = await import("../src/engine/actions")
  const [state, set] = createEngineState()
  const originalFetch = globalThis.fetch
  let request: Request | undefined
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    request = input instanceof Request ? input : new Request(input, init)
    return new Response(null, { status: 204 })
  }) as typeof fetch
  try {
    const actions = createActions(
      () => {
        throw new Error("active workspace client should not be used")
      },
      state,
      set,
      () => ({ url: "http://engine.test" }),
    )
    expect(
      await actions.recover("child", {
        model: { providerID: "openai", modelID: "gpt-5" },
        agent: "explore",
        directory: "D:/work/beta",
      }),
    ).toEqual({ ok: true })
    const url = new URL(request!.url)
    expect(url.pathname).toBe("/session/child/prompt_async")
    expect(decodeURIComponent(request!.headers.get("x-opencode-directory")!)).toBe("D:/work/beta")
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("parent delegated status follows interruption, resumed work, and completion", async () => {
  const { delegatedTaskStatus } = await import("../src/ui/parts")
  const [state, set] = createEngineState()
  const part = {
    sessionID: "parent",
    state: { status: "completed", input: {}, output: '<task id="child" state="running">' },
  } as never
  recordRecoverableInterruption(
    {
      ...interruption(),
      kind: "usage",
      reason: "usage limit reached",
    },
    store,
  )
  expect(delegatedTaskStatus(state, part, "child")).toBe("interrupted")
  clearRecoverableInterruption("child", true, store)
  set("status", "child", { type: "busy" })
  expect(delegatedTaskStatus(state, part, "child")).toBe("resumed")
  set("status", "child", { type: "idle" })
  expect(delegatedTaskStatus(state, part, "child")).toBe("completed")
})

test("running delegated rows navigate while terminal rows expand without lifecycle badges", async () => {
  const { delegatedTaskClickPolicy } = await import("../src/ui/parts")
  expect(delegatedTaskClickPolicy("running", "child")).toBe("navigate")
  expect(delegatedTaskClickPolicy("resumed", "child")).toBe("navigate")
  expect(delegatedTaskClickPolicy("completed", "child")).toBe("expand")
  expect(delegatedTaskClickPolicy("error", "child")).toBe("expand")
  const source = await Bun.file("src/ui/parts.tsx").text()
  expect(source).not.toContain("drift.recovery.task.")
  expect(source).toContain("selectSession(spawnedId()!)")
})

test("failed recovery keeps the interruption actionable and updates its model and reason", async () => {
  const { createActions } = await import("../src/engine/actions")
  recordRecoverableInterruption(
    {
      ...interruption(),
      kind: "usage",
      reason: "usage limit reached",
    },
    store,
  )
  const [state, set] = createEngineState()
  const client = { session: { promptAsync: async () => ({ error: { data: { message: "provider unavailable" } } }) } }
  const actions = createActions(() => client as never, state, set, () => undefined)
  const result = await actions.recover("child", {
    model: { providerID: "openai", modelID: "gpt-5" },
    agent: "explore",
  })
  expect(result.ok).toBeFalse()
  expect(recoverableForSession("child")).toMatchObject({
    providerId: "openai",
    modelId: "gpt-5",
    reason: "Recovery failed: provider unavailable",
  })
})
