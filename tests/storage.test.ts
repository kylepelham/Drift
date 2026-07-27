import { expect, test } from "bun:test"

// Matches the polyfill the other suites install; persisted() reads localStorage at module scope.
if (!("localStorage" in globalThis)) {
  const store = new Map<string, string>()
  Object.defineProperty(globalThis, "localStorage", {
    value: {
      getItem: (key: string) => store.get(key) ?? null,
      setItem: (key: string, value: string) => void store.set(key, value),
    },
  })
}

test("byte sizes are formatted to the nearest sensible unit", async () => {
  const { formatBytes } = await import("../src/state/storage")
  expect(formatBytes(0)).toBe("0 B")
  expect(formatBytes(-1)).toBe("0 B")
  expect(formatBytes(512)).toBe("512 B")
  expect(formatBytes(1024)).toBe("1.0 KB")
  expect(formatBytes(1024 * 1024 * 1.5)).toBe("1.5 MB")
  expect(formatBytes(1024 ** 3 * 11.7)).toBe("11.7 GB")
  // Three-digit values drop the decimal so the column stays narrow.
  expect(formatBytes(1024 ** 2 * 512)).toBe("512 MB")
})

test("cleanup defaults only enable rules that cannot lose readable history", async () => {
  const { defaultStorageRules } = await import("../src/state/storage")
  expect(defaultStorageRules.supersededSnapshots).toBeTrue()
  expect(defaultStorageRules.orphanEvents).toBeTrue()
  // Dropping a subagent's or an archived thread's history is defensible but is the user's call.
  expect(defaultStorageRules.subagentEvents).toBeFalse()
  expect(defaultStorageRules.archivedEvents).toBeFalse()
})

test("a rule toggle preserves the other rules", async () => {
  const { setStorageRule, storageRules, anyRuleEnabled } = await import("../src/state/storage")
  setStorageRule("subagentEvents", true)
  expect(storageRules().subagentEvents).toBeTrue()
  expect(storageRules().supersededSnapshots).toBeTrue()
  setStorageRule("supersededSnapshots", false)
  setStorageRule("orphanEvents", false)
  setStorageRule("subagentEvents", false)
  expect(anyRuleEnabled(storageRules())).toBeFalse()
  setStorageRule("orphanEvents", true)
  expect(anyRuleEnabled(storageRules())).toBeTrue()
})

test("scheduled cleanup stays off unless enabled, and runs at most daily", async () => {
  const storage = await import("../src/state/storage")
  const dayMs = 24 * 60 * 60 * 1000
  storage.setStorageRule("orphanEvents", true)

  // Off by default: nothing should be stamped.
  storage.setAutoCleanup(false)
  storage.setLastCleanupAt(0)
  await storage.runScheduledCleanup(dayMs * 10)
  expect(storage.lastCleanupAt()).toBe(0)

  // Enabled and never run: the attempt is stamped even though there is no desktop backend here.
  storage.setAutoCleanup(true)
  await storage.runScheduledCleanup(dayMs * 10)
  expect(storage.lastCleanupAt()).toBe(dayMs * 10)

  // Within a day of the last run it must not run again.
  await storage.runScheduledCleanup(dayMs * 10 + 1000)
  expect(storage.lastCleanupAt()).toBe(dayMs * 10)

  // A day later it runs.
  await storage.runScheduledCleanup(dayMs * 11 + 1)
  expect(storage.lastCleanupAt()).toBe(dayMs * 11 + 1)
})

test("no enabled rules means scheduled cleanup does nothing", async () => {
  const storage = await import("../src/state/storage")
  storage.setAutoCleanup(true)
  storage.setLastCleanupAt(0)
  storage.setStorageRules({
    supersededSnapshots: false,
    subagentEvents: false,
    archivedEvents: false,
    orphanEvents: false,
  })
  await storage.runScheduledCleanup(999_999_999)
  expect(storage.lastCleanupAt()).toBe(0)
})
