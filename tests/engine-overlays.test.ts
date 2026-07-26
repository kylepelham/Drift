import { expect, test } from "bun:test"
import { spawn } from "node:child_process"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import {
  acquireEngineOverlayLock,
  releaseEngineOverlayLock,
  runWithEngineOverlays,
  type EngineOverlayOperations,
} from "../scripts/engine-overlays"

const lockWorker = path.join(import.meta.dirname, "fixtures", "engine-overlay-lock-worker.ts")
const workerErrors = new WeakMap<ReturnType<typeof spawn>, Buffer[]>()

function exists(file: string) {
  return existsSync(file)
}

async function waitFor(file: string, timeout = 5_000) {
  const deadline = Date.now() + timeout
  while (Date.now() < deadline) {
    if (exists(file)) return
    await Bun.sleep(20)
  }
  throw new Error(`Timed out waiting for ${file}`)
}

function worker(input: Record<string, unknown>) {
  const child = spawn(process.execPath, [lockWorker, JSON.stringify(input)], { stdio: "pipe" })
  const errors: Buffer[] = []
  child.stderr?.on("data", (data) => errors.push(Buffer.from(data)))
  workerErrors.set(child, errors)
  return child
}

function assertRunning(child: ReturnType<typeof worker>) {
  if (child.exitCode === null) return
  throw new Error(Buffer.concat(workerErrors.get(child) ?? []).toString() || `Lock worker exited ${child.exitCode}`)
}

async function waitForExit(child: ReturnType<typeof worker>) {
  if (child.exitCode !== null || child.signalCode !== null) return
  await new Promise<void>((resolve) => child.once("close", () => resolve()))
}

async function stop(child: ReturnType<typeof worker>) {
  if (child.exitCode !== null || child.signalCode !== null) return
  const closed = new Promise<void>((resolve) => child.once("close", () => resolve()))
  if (process.platform === "win32" && child.pid) {
    await new Promise<void>((resolve) =>
      spawn("taskkill", ["/pid", String(child.pid), "/T", "/F"]).once("close", () => resolve()),
    )
  }
  child.kill()
  await closed
}

function acquired(file: string) {
  try {
    return readFileSync(file, "utf8").split(/\r?\n/).filter(Boolean)
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return []
    throw error
  }
}

function candidates(directory: string) {
  return readdirSync(directory).filter((entry) => entry.includes("-candidate-"))
}

function messages(error: unknown): string[] {
  if (error instanceof AggregateError) return [error.message, ...error.errors.flatMap(messages)]
  return [error instanceof Error ? error.message : String(error)]
}

function fixture(
  overlays = ["one.patch"],
  options: { applied?: string[]; dirty?: string[]; failedReversals?: string[] } = {},
) {
  const state = new Map(
    overlays.map((overlay) => [
      overlay,
      options.dirty?.includes(overlay) ? "dirty" : options.applied?.includes(overlay) ? "applied" : "absent",
    ]),
  )
  const events: string[] = []
  const failedReversals = new Set(options.failedReversals)
  const operations: EngineOverlayOperations = {
    overlays,
    acquireLock: async () => {
      events.push("lock")
    },
    releaseLock: () => {
      events.push("unlock")
    },
    canApply: async (overlay, reverse) => state.get(overlay) === (reverse ? "applied" : "absent"),
    apply: async (overlay, reverse) => {
      events.push(`${reverse ? "reverse" : "apply"}:${overlay}`)
      if (reverse && failedReversals.has(overlay)) return false
      state.set(overlay, reverse ? "absent" : "applied")
      return true
    },
    upstreamChanges: async () =>
      [...state]
        .filter(([, value]) => value !== "absent")
        .map(([overlay]) => ` M engine/upstream/${overlay}`),
  }
  return { events, operations }
}

test("successful callback fails and retains the lock when cleanup fails", async () => {
  const { events, operations } = fixture(["one.patch"], { failedReversals: ["one.patch"] })

  await expect(runWithEngineOverlays(async () => "ok", operations)).rejects.toThrow("Engine overlay restoration failed")
  expect(events).toContain("reverse:one.patch")
  expect(events).not.toContain("unlock")
})

test("callback and cleanup errors are both preserved", async () => {
  const callbackError = new Error("callback failed")
  const { operations } = fixture(["one.patch"], { failedReversals: ["one.patch"] })

  try {
    await runWithEngineOverlays(async () => {
      throw callbackError
    }, operations)
    throw new Error("expected command failure")
  } catch (error) {
    expect(error).toBeInstanceOf(AggregateError)
    expect((error as AggregateError).errors[0]).toBe(callbackError)
    expect(messages(error)).toContain("callback failed")
    expect(messages(error)).toContain("Failed to restore one.patch")
  }
})

test("callback failure releases the lock after clean restoration", async () => {
  const callbackError = new Error("callback failed")
  const { events, operations } = fixture()

  await expect(
    runWithEngineOverlays(async () => {
      throw callbackError
    }, operations),
  ).rejects.toBe(callbackError)
  expect(events).toContain("unlock")
})

test("cleanup attempts every reversal in reverse order", async () => {
  const { events, operations } = fixture(["one.patch", "two.patch", "three.patch"], {
    failedReversals: ["one.patch", "three.patch"],
  })

  await expect(runWithEngineOverlays(async () => undefined, operations)).rejects.toThrow()
  expect(events.filter((event) => event.startsWith("reverse:"))).toEqual([
    "reverse:three.patch",
    "reverse:two.patch",
    "reverse:one.patch",
  ])
})

test("unrecoverable startup state does not run the callback or release the lock", async () => {
  const { events, operations } = fixture(["dirty.patch"], { dirty: ["dirty.patch"] })
  let called = false

  try {
    await runWithEngineOverlays(async () => {
      called = true
    }, operations)
    throw new Error("expected recovery failure")
  } catch (error) {
    expect(messages(error)).toContain(
      "Startup recovery left engine/upstream dirty; manual recovery is required:\n M engine/upstream/dirty.patch",
    )
  }
  expect(called).toBe(false)
  expect(events).not.toContain("unlock")
})

test("startup reverses a recoverable interrupted overlay before running", async () => {
  const { events, operations } = fixture(["one.patch"], { applied: ["one.patch"] })

  expect(await runWithEngineOverlays(async () => "ok", operations)).toBe("ok")
  expect(events).toEqual([
    "lock",
    "reverse:one.patch",
    "apply:one.patch",
    "reverse:one.patch",
    "unlock",
  ])
})

test("clean success restores overlays and releases the lock", async () => {
  const { events, operations } = fixture(["one.patch", "two.patch"])

  expect(await runWithEngineOverlays(async () => "result", operations)).toBe("result")
  expect(events).toEqual([
    "lock",
    "apply:one.patch",
    "apply:two.patch",
    "reverse:two.patch",
    "reverse:one.patch",
    "unlock",
  ])
})

test("an owner write failure never publishes a visible lock", async () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), "drift-overlay-owner-write-"))
  const lock = path.join(directory, "lock")
  const failure = new Error("injected owner write failure")

  try {
    await expect(
      acquireEngineOverlayLock(lock, {
        beforeOwnerWrite: () => {
          throw failure
        },
      }),
    ).rejects.toBe(failure)
    expect(exists(lock)).toBe(false)
    expect(candidates(directory)).toEqual([])
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})

test("a crash after owner persistence but before publication leaves no visible lock", async () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), "drift-overlay-publish-crash-"))
  const lock = path.join(directory, "lock")
  const crashed = path.join(directory, "crashed")
  const child = worker({
    lock,
    acquired: path.join(directory, "acquired"),
    ready: path.join(directory, "ready"),
    crashBeforePublish: crashed,
    holdMs: 0,
  })

  try {
    await waitFor(crashed)
    await waitForExit(child)
    expect(child.exitCode).toBe(70)
    expect(exists(lock)).toBe(false)
    expect(candidates(directory)).toHaveLength(1)

    await acquireEngineOverlayLock(lock)
    releaseEngineOverlayLock(lock)
    expect(exists(lock)).toBe(false)
  } finally {
    await stop(child)
    rmSync(directory, { recursive: true, force: true })
  }
})

test("two processes racing initial publication admit exactly one writer", async () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), "drift-overlay-publish-race-"))
  const lock = path.join(directory, "lock")
  const synchronization = path.join(directory, "publish-ready")
  const acquisitions = path.join(directory, "acquired")
  mkdirSync(synchronization)

  const first = worker({
    lock,
    acquired: acquisitions,
    ready: path.join(directory, "first-ready"),
    synchronizePublish: synchronization,
    holdMs: 20_000,
  })
  const second = worker({
    lock,
    acquired: acquisitions,
    ready: path.join(directory, "second-ready"),
    synchronizePublish: synchronization,
    holdMs: 20_000,
  })

  try {
    await waitFor(acquisitions)
    await Bun.sleep(500)
    expect(acquired(acquisitions)).toHaveLength(1)
    expect(
      [exists(path.join(directory, "first-ready")), exists(path.join(directory, "second-ready"))].filter(Boolean),
    ).toHaveLength(1)
    assertRunning(first)
    assertRunning(second)
  } finally {
    await Promise.all([stop(first), stop(second)])
    rmSync(directory, { recursive: true, force: true })
  }
}, 10_000)

test("a live lock excludes another process", async () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), "drift-overlay-lock-"))
  const lock = path.join(directory, "lock")
  const acquisitions = path.join(directory, "acquired")
  const firstReady = path.join(directory, "first-ready")
  const secondReady = path.join(directory, "second-ready")
  const first = worker({ lock, acquired: acquisitions, ready: firstReady, holdMs: 20_000 })
  let second: ReturnType<typeof worker> | undefined

  try {
    await waitFor(firstReady)
    second = worker({ lock, acquired: acquisitions, ready: secondReady, holdMs: 20_000 })
    await Bun.sleep(500)
    expect(acquired(acquisitions)).toHaveLength(1)
    expect(exists(secondReady)).toBe(false)
    assertRunning(first)
    assertRunning(second)
  } finally {
    await Promise.all([stop(first), second ? stop(second) : Promise.resolve()])
    rmSync(directory, { recursive: true, force: true })
  }
}, 10_000)

test("two processes racing stale takeover admit exactly one writer", async () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), "drift-overlay-stale-lock-"))
  const lock = path.join(directory, "lock")
  const synchronization = path.join(directory, "dead-checks")
  const acquisitions = path.join(directory, "acquired")
  mkdirSync(lock)
  mkdirSync(synchronization)

  const exited = spawn(process.execPath, ["-e", ""])
  await new Promise<void>((resolve) => exited.once("close", () => resolve()))
  writeFileSync(
    path.join(lock, "owner.json"),
    JSON.stringify({ pid: exited.pid, token: "00000000-0000-4000-8000-000000000001" }),
  )

  const first = worker({
    lock,
    acquired: acquisitions,
    ready: path.join(directory, "first-ready"),
    synchronizeDeadCheck: synchronization,
    holdMs: 20_000,
  })
  const second = worker({
    lock,
    acquired: acquisitions,
    ready: path.join(directory, "second-ready"),
    synchronizeDeadCheck: synchronization,
    holdMs: 20_000,
  })

  try {
    await waitFor(acquisitions)
    await Bun.sleep(500)
    expect(acquired(acquisitions)).toHaveLength(1)
    expect(exists(path.join(`${lock}-reclaimed`, "00000000-0000-4000-8000-000000000001"))).toBe(true)
    assertRunning(first)
    assertRunning(second)
  } finally {
    await Promise.all([stop(first), stop(second)])
    rmSync(directory, { recursive: true, force: true })
  }
}, 10_000)

test("a crash between stale quarantine and claim leaves the lock claimable", async () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), "drift-overlay-reclaim-crash-"))
  const lock = path.join(directory, "lock")
  const tombstone = path.join(`${lock}-reclaimed`, "00000000-0000-4000-8000-000000000001")
  mkdirSync(tombstone, { recursive: true })
  writeFileSync(
    path.join(tombstone, "owner.json"),
    JSON.stringify({ pid: process.pid + 1, token: "00000000-0000-4000-8000-000000000001" }),
  )

  try {
    await acquireEngineOverlayLock(lock)
    expect(exists(lock)).toBe(true)
    releaseEngineOverlayLock(lock)
    expect(exists(lock)).toBe(false)
    expect(exists(tombstone)).toBe(true)
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})

test("an empty ownerless legacy lock is quarantined only when upstream is clean", async () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), "drift-overlay-ownerless-clean-"))
  const lock = path.join(directory, "lock")
  mkdirSync(lock)

  try {
    await acquireEngineOverlayLock(lock, { upstreamChanges: async () => [] })
    expect(exists(path.join(`${lock}-reclaimed`, "ownerless-legacy"))).toBe(true)
    expect(JSON.parse(readFileSync(lock, "utf8"))).toMatchObject({ pid: process.pid })
    releaseEngineOverlayLock(lock)
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})

test("a legacy pid lock held by a live process keeps blocking", async () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), "drift-overlay-legacy-live-"))
  const lock = path.join(directory, "lock")
  const acquisitions = path.join(directory, "acquired")
  const ready = path.join(directory, "ready")
  mkdirSync(lock)
  writeFileSync(path.join(lock, "pid"), `${process.pid}`)
  const child = worker({ lock, acquired: acquisitions, ready, holdMs: 20_000 })

  try {
    await Bun.sleep(1_000)
    expect(acquired(acquisitions)).toEqual([])
    expect(exists(ready)).toBe(false)
    expect(readFileSync(path.join(lock, "pid"), "utf8")).toBe(`${process.pid}`)
    expect(exists(`${lock}-reclaimed`)).toBe(false)
    assertRunning(child)
  } finally {
    await stop(child)
    rmSync(directory, { recursive: true, force: true })
  }
}, 10_000)

test("a legacy pid lock left by a dead process is reclaimed", async () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), "drift-overlay-legacy-dead-"))
  const lock = path.join(directory, "lock")
  mkdirSync(lock)

  const exited = spawn(process.execPath, ["-e", ""])
  await new Promise<void>((resolve) => exited.once("close", () => resolve()))
  writeFileSync(path.join(lock, "pid"), `${exited.pid}`)
  const reclaimed = path.join(`${lock}-reclaimed`, `legacy-pid-${exited.pid}`)

  try {
    await acquireEngineOverlayLock(lock)
    expect(readFileSync(path.join(reclaimed, "pid"), "utf8")).toBe(`${exited.pid}`)
    expect(JSON.parse(readFileSync(lock, "utf8"))).toMatchObject({ pid: process.pid })
    releaseEngineOverlayLock(lock)
    expect(exists(lock)).toBe(false)
    expect(exists(reclaimed)).toBe(true)
    expect(candidates(directory)).toEqual([])
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})

test("a malformed or crowded legacy pid lock fails closed", async () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), "drift-overlay-legacy-invalid-"))
  const malformed = path.join(directory, "malformed")
  const crowded = path.join(directory, "crowded")
  mkdirSync(malformed)
  writeFileSync(path.join(malformed, "pid"), "0x1f")
  mkdirSync(crowded)
  writeFileSync(path.join(crowded, "pid"), `${process.pid}`)
  writeFileSync(path.join(crowded, "notes.txt"), "")

  try {
    await expect(acquireEngineOverlayLock(malformed, { upstreamChanges: async () => [] })).rejects.toThrow(
      "Invalid engine overlay lock owner",
    )
    await expect(acquireEngineOverlayLock(crowded, { upstreamChanges: async () => [] })).rejects.toThrow(
      "Ownerless engine overlay lock contains unexpected files",
    )
    for (const lock of [malformed, crowded]) {
      expect(readFileSync(path.join(lock, "pid"), "utf8")).not.toBe("")
      expect(exists(`${lock}-reclaimed`)).toBe(false)
    }
    expect(candidates(directory)).toEqual([])
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
}, 10_000)

test("an ownerless legacy lock fails closed when upstream is dirty", async () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), "drift-overlay-ownerless-dirty-"))
  const lock = path.join(directory, "lock")
  mkdirSync(lock)

  try {
    await expect(
      acquireEngineOverlayLock(lock, {
        upstreamChanges: async () => [" M engine/upstream/dirty.ts"],
      }),
    ).rejects.toThrow("Ownerless engine overlay lock cannot be recovered while engine/upstream is dirty")
    expect(exists(lock)).toBe(true)
    expect(exists(path.join(`${lock}-reclaimed`, "ownerless-legacy"))).toBe(false)
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})
