import { expect, test } from "bun:test"

const at = (y: number, m: number, d: number, h = 12) => new Date(y, m - 1, d, h).getTime()

test("each day's first thread carries a heading, newest first", async () => {
  const { dayDividers } = await import("../src/ui/workspaces")
  const now = at(2026, 10, 7, 15)
  const rows = [
    { id: "a", updated: at(2026, 10, 7, 14) },
    { id: "b", updated: at(2026, 10, 7, 9) },
    { id: "c", updated: at(2026, 10, 6, 23) },
    { id: "d", updated: at(2026, 10, 3) },
    { id: "e", updated: at(2026, 9, 20) },
    { id: "f", updated: at(2025, 12, 31) },
  ]
  const headings = dayDividers(rows, now)
  expect([...headings.keys()]).toEqual(["a", "c", "d", "e", "f"])
  expect(headings.get("a")).toBe("Today")
  expect(headings.get("c")).toBe("Yesterday")
  expect(headings.get("d")).toBe(new Date(at(2026, 10, 3)).toLocaleDateString(undefined, { weekday: "long" }))
  expect(headings.get("e")).toBe(new Date(at(2026, 9, 20)).toLocaleDateString(undefined, { day: "numeric", month: "short" }))
  expect(headings.get("f"), "another year names its year").toContain("2025")
})

test("a thread active just after midnight is today's, one just before is yesterday's", async () => {
  const { dayLabel } = await import("../src/ui/workspaces")
  const now = at(2026, 10, 7, 0)
  expect(dayLabel(new Date(2026, 9, 7, 0, 1).getTime(), now)).toBe("Today")
  expect(dayLabel(new Date(2026, 9, 6, 23, 59).getTime(), now)).toBe("Yesterday")
})
