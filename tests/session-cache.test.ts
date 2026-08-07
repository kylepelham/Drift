import { expect, test } from "bun:test"

const storage = new Map<string, string>()
localStorage.getItem = (key: string) => storage.get(key) ?? null
localStorage.setItem = (key: string, value: string) => storage.set(key, value)

test("cached threads survive a restart and only the engine may replace them", async () => {
  const { cachedSessions, cachedSessionLimit, forgetCachedSessions, rememberSessions } = await import(
    "../src/state/session-cache"
  )
  const directory = "C:/work/drift"
  expect(cachedSessions(directory)).toEqual([])

  rememberSessions(directory, [{ id: "one", title: "First", updated: 2 }])
  expect(cachedSessions(directory)).toEqual([{ id: "one", title: "First", updated: 2 }])
  expect(JSON.parse(localStorage.getItem("drift.sessions.cache")!)[directory]).toHaveLength(1)
  rememberSessions(directory, [{ id: "one", title: "First", updated: 3 }])
  expect(cachedSessions(directory)[0]?.updated).toBe(3)

  // An authoritative empty list clears the workspace instead of stranding deleted threads.
  rememberSessions(directory, [])
  expect(cachedSessions(directory)).toEqual([])

  const many = Array.from({ length: cachedSessionLimit + 10 }, (_, index) => ({
    id: `session-${index}`,
    title: `Thread ${index}`,
    updated: index,
  }))
  rememberSessions(directory, many)
  expect(cachedSessions(directory)).toHaveLength(cachedSessionLimit)

  forgetCachedSessions(directory)
  expect(cachedSessions(directory)).toEqual([])
})

test("a stored cache is restored and malformed entries are discarded", async () => {
  const { normalizeSessionCache, cachedSessionLimit } = await import("../src/state/session-cache")
  expect(normalizeSessionCache(null)).toEqual({})
  expect(normalizeSessionCache(["not a map"])).toEqual({})
  expect(
    normalizeSessionCache({
      "C:/work": [
        { id: "keep", title: "Keep", updated: 5 },
        { id: "missing-updated", title: "Broken" },
        { id: 7, title: "Wrong type", updated: 1 },
        null,
      ],
      "C:/empty": [],
    }),
  ).toEqual({ "C:/work": [{ id: "keep", title: "Keep", updated: 5 }] })
  expect(
    normalizeSessionCache({
      "C:/work": Array.from({ length: cachedSessionLimit + 5 }, (_, index) => ({
        id: `${index}`,
        title: `${index}`,
        updated: index,
      })),
    })["C:/work"],
  ).toHaveLength(cachedSessionLimit)
})
