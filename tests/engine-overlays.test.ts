import { expect, test } from "bun:test"
import {
  runWithEngineOverlays,
  type EngineOverlayOperations,
} from "../scripts/engine-overlays"

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
