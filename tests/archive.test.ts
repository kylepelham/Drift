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
