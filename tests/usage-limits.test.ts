import { afterEach, beforeEach, expect, mock, test } from "bun:test"
import type { MessageEntry } from "../src/engine/store"
import { estimateContextBreakdown } from "../src/engine/context-breakdown"
import { refreshUsage, resetLabel, resetTitle, usageFor, usageTone, windowLabel } from "../src/state/usage-limits"

const minute = 60_000
const now = Date.UTC(2026, 8, 28, 12, 0)
let invoke: ReturnType<typeof mock>
let original: PropertyDescriptor | undefined

beforeEach(() => {
  original = Object.getOwnPropertyDescriptor(globalThis, "__TAURI__")
  invoke = mock(async () => ({ status: "ok", plan: "max", windows: [{ kind: "session", label: null, usedPercent: 91, resetsAt: now + 49 * minute }] }))
  Object.defineProperty(globalThis, "__TAURI__", { configurable: true, writable: true, value: { core: { invoke } } })
})

afterEach(() => {
  if (original) Object.defineProperty(globalThis, "__TAURI__", original)
  else delete (globalThis as Record<string, unknown>).__TAURI__
})

test("limit bars turn amber at 70 percent and red at 90 percent", () => {
  expect(usageTone(0)).toBe("normal")
  expect(usageTone(69.9)).toBe("normal")
  expect(usageTone(70)).toBe("warn")
  expect(usageTone(89)).toBe("warn")
  expect(usageTone(90)).toBe("danger")
  expect(usageTone(100)).toBe("danger")
})

test("reset labels always count down and the hover title names the exact time", () => {
  expect(resetLabel(null, now)).toBe("")
  expect(resetLabel(now - 1, now)).toBe("Resetting now")
  expect(resetLabel(now + 49 * minute, now)).toBe("Resets in 49 min")
  expect(resetLabel(now + 20 * 1000, now)).toBe("Resets in 1 min")
  expect(resetLabel(now + (3 * 60 + 12) * minute, now)).toBe("Resets in 3 hr 12 min")
  expect(resetLabel(now + (3 * 24 * 60 + 5 * 60 + 30) * minute, now)).toBe("Resets in 3 d 5 hr")
  expect(resetTitle(null)).toBeUndefined()
  expect(resetTitle(now)).toMatch(/^Resets \S+/)
})

test("window labels follow the window kind and any model scope", () => {
  const base = { usedPercent: 0, resetsAt: null }
  expect(windowLabel({ ...base, kind: "session", label: null })).toBe("5-hour limit")
  expect(windowLabel({ ...base, kind: "weekly", label: null })).toBe("Weekly · all models")
  expect(windowLabel({ ...base, kind: "weekly", label: "Opus" })).toBe("Weekly · Opus")
  expect(windowLabel({ ...base, kind: "monthly", label: "premium" })).toBe("Premium requests")
  expect(windowLabel({ ...base, kind: "period", label: null })).toBe("Billing period")
})

test("usage is fetched once per minute per provider and keeps the last value on failure", async () => {
  await refreshUsage("anthropic")
  expect(invoke.mock.calls).toEqual([["provider_usage", { provider: "anthropic" }]])
  expect(usageFor("anthropic")?.usage?.plan).toBe("max")
  await refreshUsage("anthropic")
  expect(invoke).toHaveBeenCalledTimes(1)

  invoke.mockRejectedValue(new Error("usage request failed (429)"))
  await refreshUsage("anthropic", Date.now() + 2 * minute)
  expect(invoke).toHaveBeenCalledTimes(2)
  expect(usageFor("anthropic")).toMatchObject({ failed: true, loading: false, usage: { plan: "max" } })
})

test("providers without plan windows resolve to null, which hides the section", async () => {
  invoke.mockResolvedValue(null)
  await refreshUsage("openrouter")
  expect(usageFor("openrouter")).toMatchObject({ usage: null, failed: false })
})

function entry(role: "user" | "assistant", parts: unknown[], extra: Record<string, unknown> = {}): MessageEntry {
  return { info: { id: `${role}-${Math.random()}`, role, time: { created: 0 }, ...extra }, parts } as unknown as MessageEntry
}

test("the breakdown estimates transcript categories and attributes the rest to system and tools", () => {
  const entries = [
    entry("user", [{ type: "text", text: "x".repeat(400) }]),
    entry("assistant", [
      { type: "text", text: "y".repeat(800) },
      { type: "tool", state: { status: "completed", input: { path: "a" }, output: "z".repeat(1184) } },
    ]),
  ]
  expect(estimateContextBreakdown(entries, 10_000)).toEqual([
    { key: "system", tokens: 9_400 },
    { key: "user", tokens: 100 },
    { key: "assistant", tokens: 200 },
    { key: "tool", tokens: 300 },
  ])
})

test("the breakdown ignores messages before the latest compaction and scales down overestimates", () => {
  const entries = [
    entry("user", [{ type: "text", text: "old".repeat(10_000) }]),
    entry("assistant", [{ type: "text", text: "s".repeat(400) }], { summary: true }),
    entry("user", [{ type: "text", text: "u".repeat(400) }]),
  ]
  expect(estimateContextBreakdown(entries, 1_000)).toEqual([
    { key: "system", tokens: 800 },
    { key: "user", tokens: 100 },
    { key: "assistant", tokens: 100 },
  ])
  const scaled = estimateContextBreakdown(entries, 100)
  expect(scaled.reduce((sum, segment) => sum + segment.tokens, 0)).toBe(100)
  expect(estimateContextBreakdown(entries, 0)).toEqual([])
})

test("the context meter shows usage limits and the breakdown, and remote access can ask for usage", async () => {
  const meter = await Bun.file("src/ui/context-meter.tsx").text()
  expect(meter).toContain("data-context-bar")
  const { dict, drift } = await import("../src/i18n/en")
  const english: Record<string, string> = { ...dict, ...drift }
  const keys = [...meter.matchAll(/"((?:drift|context)\.[a-zA-Z.]+)"/g)].map((match) => match[1]!)
  expect(keys.length).toBeGreaterThan(5)
  expect(keys.filter((key) => !(key in english))).toEqual([])
  expect(meter).toContain("<UsageSection provider=")
  expect(meter).toContain("<ProviderIcon id={props.provider}")
  expect(meter).not.toContain("detailedBreakdown")
  expect(meter).toContain("onMouseEnter={refresh}")
  expect(await Bun.file("src/ui/header.tsx").text()).toContain("<ContextMeter sessionId=")
  expect(await Bun.file("src/ui/debug.tsx").text()).toContain("<ContextSection sessionId=")
  const remote = await Bun.file("src-tauri/src/remote.rs").text()
  expect(remote).toContain('"provider_usage" => value(crate::usage_limits::provider_usage(app.state(), arg(args, "provider")?).await?)')
  expect(await Bun.file("src-tauri/src/main.rs").text()).toContain("usage_limits::provider_usage,")
})

test("settings lists usage for every linked provider and forced refresh skips the one-minute cache", async () => {
  await refreshUsage("zai-coding-plan")
  const calls = invoke.mock.calls.length
  await refreshUsage("zai-coding-plan")
  expect(invoke.mock.calls.length).toBe(calls)
  await refreshUsage("zai-coding-plan", Date.now(), true)
  expect(invoke.mock.calls.length).toBe(calls + 1)
  const settings = await Bun.file("src/ui/settings.tsx").text()
  expect(settings).toContain('items: ["Tools", "Providers", "Usage", "MCP", "Prompts", "Permissions"]')
  expect(settings).toContain("<UsageLimitsSection />")
  const section = await Bun.file("src/ui/settings-usage.tsx").text()
  expect(section).toContain("engine.state.connected.includes(provider.id)")
  expect(section).toContain("usageFor(provider.id)?.usage !== null")
})
