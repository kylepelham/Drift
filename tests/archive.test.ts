import { expect, test } from "bun:test"

const storage = new Map<string, string>()
if (!("localStorage" in globalThis)) {
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => storage.set(key, value),
      removeItem: (key: string) => storage.delete(key),
    },
  })
}
localStorage.getItem = (key: string) => storage.get(key) ?? null
localStorage.setItem = (key: string, value: string) => void storage.set(key, value)

test("a session this browser has no choice for shows the agent and level the engine saved on it", async () => {
  const { prefsFor, updatePrefs } = await import("../src/state/prefs")
  const saved = { agent: "plan", variant: "high" }
  expect(prefsFor("ses_elsewhere", saved)).toMatchObject(saved)
  expect(prefsFor("ses_elsewhere", { agent: "plan", variant: null }).variant).toBeNull()
  updatePrefs("ses_mine", { agent: "build", variant: "low" })
  expect(prefsFor("ses_mine", saved)).toMatchObject({ agent: "build", variant: "low" })
})

test("an unsent edit wins, then the session's own choice and model, then the global default; sending clears the edit", async () => {
  const { clearEdits, prefsFor, sendableVariant, updatePrefs } = await import("../src/state/prefs")
  const sessionModel = { providerID: "anthropic", modelID: "claude-sonnet-4-5" }
  const saved = { agent: "plan", variant: "high", model: sessionModel }
  const edited = { providerID: "openai", modelID: "gpt-5" }
  updatePrefs("ses_edit", { model: edited, variant: "low" })
  expect(prefsFor("ses_edit", saved)).toEqual({ model: edited, agent: "plan", variant: "low" })
  expect(prefsFor("ses_other", saved).model).toEqual(sessionModel)
  expect(prefsFor("ses_other", {}).model).toEqual(edited)
  clearEdits("ses_edit")
  expect(prefsFor("ses_edit", saved)).toEqual({ model: sessionModel, agent: "plan", variant: "high" })
  expect([sendableVariant("high", ["low", "high"]), sendableVariant(null, ["high"]), sendableVariant("max", ["high"]), sendableVariant(undefined, [])]).toEqual([
    "high",
    null,
    undefined,
    undefined,
  ])
})

const long = Date.now() - 30 * 24 * 60 * 60 * 1000

function archivedLongAgo(...sessionIds: string[]) {
  storage.set("drift.store.archived", JSON.stringify(sessionIds.map((sessionId) => ({ sessionId, workspaceId: "w1", archivedAt: long }))))
}

test("the purge drops a record when the engine deleted the thread or kept it as restored, and retries one it could not reach", async () => {
  const { archivedIds, purgeArchived } = await import("../src/state/workspaces")
  archivedLongAgo("gone", "restored", "unreachable")
  const outcomes = { gone: "deleted", restored: "kept", unreachable: "failed" } as const
  const complete = await purgeArchived(async (sessionId) => outcomes[sessionId as keyof typeof outcomes])
  expect(complete).toBeFalse()
  expect([...archivedIds()]).toEqual(["unreachable"])
})

test("a purge waits for a restore under way, so it never acts on a half-restored thread", async () => {
  const { purgeArchived, unarchiveSession } = await import("../src/state/workspaces")
  archivedLongAgo("ses_9")
  const order: string[] = []
  let finishRestore!: () => void
  const restoring = unarchiveSession("ses_9", () => new Promise<void>((resolve) => (finishRestore = () => (order.push("restored"), resolve()))))
  const purging = purgeArchived(async () => (order.push("purge asked"), "kept"))
  await new Promise((resolve) => setTimeout(resolve, 10))
  expect(order).toEqual([])
  finishRestore()
  await Promise.all([restoring, purging])
  expect(order).toEqual(["restored"])
})

test("a thread is hidden only once the engine has archived it, and returns only once the engine has restored it", async () => {
  const { archiveSession, archivedIds, unarchiveSession } = await import("../src/state/workspaces")
  const calls: [string, boolean][] = []
  const engine = async (sessionId: string, archived: boolean) => void calls.push([sessionId, archived])
  const refusing = async () => {
    throw new Error("engine offline")
  }

  await expect(archiveSession("ses_1", "w1", refusing)).rejects.toThrow("engine offline")
  expect(archivedIds().has("ses_1")).toBeFalse()

  await archiveSession("ses_1", "w1", engine)
  expect(calls).toEqual([["ses_1", true]])
  expect(archivedIds().has("ses_1")).toBeTrue()

  await expect(unarchiveSession("ses_1", refusing)).rejects.toThrow("engine offline")
  expect(archivedIds().has("ses_1")).toBeTrue()

  await unarchiveSession("ses_1", engine)
  expect(calls).toEqual([
    ["ses_1", true],
    ["ses_1", false],
  ])
  expect(archivedIds().has("ses_1")).toBeFalse()
})
