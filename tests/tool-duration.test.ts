import { describe, expect, test } from "bun:test"
import { formatToolDuration, toolElapsedMs, type ToolTimingState } from "../src/ui/tool-duration"

describe("tool duration formatting", () => {
  test.each([
    [-1, "0s"],
    [0, "0s"],
    [999, "0s"],
    [1_000, "1s"],
    [59_999, "59s"],
    [60_000, "1m 00s"],
    [198_000, "3m 18s"],
    [3_599_999, "59m 59s"],
    [3_600_000, "1h 00m"],
    [3_840_000, "1h 04m"],
  ])("formats %dms as %s", (ms, expected) => {
    expect(formatToolDuration(ms)).toBe(expected)
  })
})

describe("tool elapsed timestamps", () => {
  test("a running clock advances from its persisted start timestamp", () => {
    const state = { status: "running", time: { start: 10_000 } }
    expect(formatToolDuration(toolElapsedMs(state, 52_000)!)).toBe("42s")
    expect(formatToolDuration(toolElapsedMs(state, 53_000)!)).toBe("43s")
  })

  test("a timed-out running clock stops at the timeout", () => {
    const state = { status: "running", time: { start: 10_000 } }
    expect(toolElapsedMs(state, 1_030_000, 120_000)).toBe(120_000)
    expect(formatToolDuration(toolElapsedMs(state, 1_030_000, 120_000)!)).toBe("2m 00s")
  })

  test("a terminal clock freezes at its persisted end timestamp", () => {
    const state = { status: "completed", time: { start: 10_000, end: 52_000 } }
    expect(toolElapsedMs(state, 80_000)).toBe(42_000)
    expect(toolElapsedMs(state, 180_000)).toBe(42_000)
  })

  test("concurrent tools retain independent elapsed durations", () => {
    const first = { status: "running", time: { start: 10_000 } }
    const second = { status: "running", time: { start: 49_000 } }
    expect(formatToolDuration(toolElapsedMs(first, 52_000)!)).toBe("42s")
    expect(formatToolDuration(toolElapsedMs(second, 52_000)!)).toBe("3s")
  })

  test("reload reconstruction uses persisted timestamps and omits untimed legacy parts", () => {
    const reloaded = { status: "running", time: { start: 1_000 } }
    expect(toolElapsedMs(reloaded, 199_000)).toBe(198_000)
    expect(toolElapsedMs({ status: "running" }, 199_000)).toBeUndefined()
    expect(toolElapsedMs({ status: "error" }, 199_000)).toBeUndefined()
  })

  test.each(["error", "interrupted", "cancelled"])("freezes %s tools", (status) => {
    const state: ToolTimingState = { status, time: { start: 2_000, end: 5_000 } }
    expect(toolElapsedMs(state, 50_000)).toBe(3_000)
  })
})
