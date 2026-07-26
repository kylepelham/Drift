import { afterEach, expect, test } from "bun:test"
import type { Event } from "@opencode-ai/sdk/client"
import { streamEvents } from "../src/engine/sse"

const originalFetch = globalThis.fetch
const originalSetTimeout = globalThis.setTimeout
const originalClearTimeout = globalThis.clearTimeout

afterEach(() => {
  globalThis.fetch = originalFetch
  globalThis.setTimeout = originalSetTimeout
  globalThis.clearTimeout = originalClearTimeout
})

// A stream that never closes on its own, so only the watchdog can end the read.
function openStream() {
  let push: (frame: string) => void = () => undefined
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      push = (frame) => controller.enqueue(new TextEncoder().encode(frame))
    },
  })
  globalThis.fetch = (async () => new Response(body, { status: 200 })) as typeof fetch
  return { push }
}

function frame(payload: unknown, directory = "C:/one") {
  return `data: ${JSON.stringify({ directory, payload })}\n`
}

function trackTimers() {
  const pending = new Set<unknown>()
  globalThis.setTimeout = ((handler: TimerHandler, delay?: number, ...args: unknown[]) => {
    const id = originalSetTimeout(
      () => {
        pending.delete(id)
        ;(handler as (...rest: unknown[]) => void)(...args)
      },
      delay,
    )
    pending.add(id)
    return id
  }) as typeof setTimeout
  globalThis.clearTimeout = ((id?: unknown) => {
    pending.delete(id)
    originalClearTimeout(id as Parameters<typeof clearTimeout>[0])
  }) as typeof clearTimeout
  return pending
}

test("a silent stream trips the inactivity deadline so the reconnect loop runs", async () => {
  openStream()
  const events: Event[] = []
  const started = Date.now()
  await expect(
    streamEvents({ url: "http://engine.test" }, new AbortController().signal, (event) => events.push(event), 60),
  ).rejects.toThrow(/inactive/)
  expect(Date.now() - started).toBeLessThan(2000)
  expect(events).toHaveLength(0)
})

test("heartbeats alone keep the stream alive", async () => {
  const stream = openStream()
  const events: Event[] = []
  const controller = new AbortController()
  const pump = streamEvents({ url: "http://engine.test" }, controller.signal, (event) => events.push(event), 80)
  let settled: unknown
  void pump.then(
    () => (settled = "resolved"),
    (error) => (settled = error),
  )
  for (let i = 0; i < 8; i++) {
    stream.push(frame({ type: "server.heartbeat" }))
    await Bun.sleep(20)
  }
  // Well past the deadline had heartbeats not rearmed it, and still no delivered events.
  expect(settled).toBeUndefined()
  expect(events).toHaveLength(0)
  stream.push(frame({ type: "session.idle", properties: { sessionID: "s1" } }))
  await Bun.sleep(10)
  expect(events.map((event) => event.type)).toEqual(["session.idle"])
  controller.abort()
  await pump.catch(() => undefined)
})

test("the watchdog timer is cleared on abort and on normal exit", async () => {
  const pending = trackTimers()
  const stream = openStream()
  const controller = new AbortController()
  const pump = streamEvents({ url: "http://engine.test" }, controller.signal, () => undefined, 5_000)
  stream.push(frame({ type: "server.heartbeat" }))
  await Bun.sleep(20)
  expect(pending.size).toBe(1)
  controller.abort()
  await pump.catch(() => undefined)
  expect(pending.size).toBe(0)

  globalThis.fetch = (async () => new Response(new ReadableStream({ start: (c) => c.close() }))) as typeof fetch
  await streamEvents({ url: "http://engine.test" }, new AbortController().signal, () => undefined, 5_000)
  expect(pending.size).toBe(0)
})
