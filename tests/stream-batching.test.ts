import { afterEach, beforeEach, expect, spyOn, test } from "bun:test"
import type { Event } from "@opencode-ai/sdk/client"
import { createEffect, createMemo, createRoot, createSignal } from "solid-js"
import { streamEvents } from "../src/engine/sse"

// Bun normally resolves Solid's server build, which cannot test reactive batching.
if (process.env.DRIFT_STREAM_BATCHING_BROWSER !== "1") {
  test("stream batching with browser Solid", async () => {
    const child = Bun.spawn([process.execPath, "--conditions=browser", "test", import.meta.path], {
      env: { ...process.env, DRIFT_STREAM_BATCHING_BROWSER: "1" },
      stdout: "pipe",
      stderr: "pipe",
    })
    const [stdout, stderr, exitCode] = await Promise.all([
      new Response(child.stdout).text(),
      new Response(child.stderr).text(),
      child.exited,
    ])
    console.log(stdout.trim())
    console.log(stderr.trim())
    expect(exitCode, stderr).toBe(0)
  })
} else {
  const originalFetch = globalThis.fetch
  const originalSetTimeout = globalThis.setTimeout
  const originalClearTimeout = globalThis.clearTimeout
  const timers = new Map<number, () => void>()
  let timerID = 0
  const encoder = new TextEncoder()

  beforeEach(() => {
    globalThis.setTimeout = ((handler: () => void) => {
      const id = ++timerID
      timers.set(id, handler)
      return id
    }) as typeof setTimeout
    globalThis.clearTimeout = ((id: number) => { timers.delete(id) }) as typeof clearTimeout
  })

  afterEach(() => {
    globalThis.fetch = originalFetch
    globalThis.setTimeout = originalSetTimeout
    globalThis.clearTimeout = originalClearTimeout
    const remaining = timers.size
    timers.clear()
    expect(remaining).toBe(0)
  })

  function delta(text: string, partID = "p1") {
    return {
      type: "message.part.delta",
      properties: { sessionID: "s1", messageID: "m1", partID, field: "text", delta: text },
    }
  }

  function frame(payload: unknown, directory?: string) {
    return `data: ${JSON.stringify({ payload, directory })}\r\n\r\n`
  }

  function source(pull: (index: number, controller: ReadableStreamDefaultController<Uint8Array>) => void) {
    const abort = new AbortController()
    let reads = 0
    let cancellations = 0
    let fetchSignal: AbortSignal | undefined
    const body = new ReadableStream<Uint8Array>({
      pull(controller) { pull(reads++, controller) },
      cancel() { cancellations++ },
    }, { highWaterMark: 0 })
    globalThis.fetch = (async (_url, init) => {
      fetchSignal = init?.signal ?? undefined
      return new Response(body)
    }) as typeof fetch
    return {
      abort,
      get cancellations() { return cancellations },
      get fetchSignal() { return fetchSignal },
    }
  }

  function observe() {
    const observer = createRoot((dispose) => {
      const [text, setText] = createSignal("")
      const [control, setControl] = createSignal("initial")
      let memoRuns = 0
      const rendered = createMemo(() => {
        memoRuns++
        return text()
      })
      const seen: { text: string; control: string }[] = []
      const calls: { payload: Event; directory?: string }[] = []
      createEffect(() => { seen.push({ text: rendered(), control: control() }) })
      return {
        dispose, seen, calls, text,
        get memoRuns() { return memoRuns - 1 },
        onEvent(event: Event, directory?: string) {
          calls.push({ payload: event, directory })
          const raw = event as { type: string; properties: { delta?: string; status?: { type: string } } }
          if (raw.type === "message.part.delta") setText((value) => value + raw.properties.delta)
          else setControl(raw.properties.status?.type ?? raw.type)
        },
      }
    })
    // This also fails immediately if the subprocess resolves Solid's inert server build.
    expect(observer.seen).toEqual([{ text: "", control: "initial" }])
    observer.seen.length = 0
    return observer
  }

  test("128 callbacks cause one downstream rerun per available burst, not per delta", async () => {
    const events = Array.from({ length: 128 }, (_, i) => ({
      payload: delta(`${i},`, `p${i % 2}`),
      directory: ["C:/one", "C:/two", undefined][i % 3],
    }))
    const counts: number[] = []
    for (const chunkSize of [1, 128]) {
      const observer = observe()
      const stream = source((index, controller) => {
        // Called by the NEXT read, before supplying either more bytes or EOF.
        expect(observer.calls).toHaveLength(Math.min(index * chunkSize, events.length))
        expect(observer.seen).toHaveLength(index)
        if (index * chunkSize === events.length) return controller.close()
        controller.enqueue(encoder.encode(events.slice(index * chunkSize, (index + 1) * chunkSize)
          .map((event) => frame(event.payload, event.directory)).join("")))
      })
      try {
        await streamEvents({ url: "http://engine.test" }, stream.abort.signal, observer.onEvent)
        expect(observer.calls).toEqual(events)
        expect(observer.text()).toBe(events.map((event) => event.payload.properties.delta).join(""))
        expect(observer.memoRuns).toBe(events.length / chunkSize)
        expect(observer.seen).toHaveLength(events.length / chunkSize)
        counts.push(observer.seen.length)
        expect(stream.fetchSignal?.aborted).toBe(true)
      } finally {
        observer.dispose()
      }
    }
    console.log(`Stream reruns for 128 deltas: one delta/read=${counts[0]}, one burst/read=${counts[1]} (memos and effects)`)
  })

  test("delta bursts flush before controls and every adjacent lifecycle transition is observed", async () => {
    const observer = observe()
    const payloads = [
      delta("a"), delta("b"),
      { type: "session.status", properties: { sessionID: "s1", status: { type: "busy" } } },
      { type: "session.status", properties: { sessionID: "s1", status: { type: "idle" } } },
      { type: "permission.asked", properties: { sessionID: "s1", id: "permission1" } },
      { type: "permission.replied", properties: { sessionID: "s1", requestID: "permission1" } },
      delta("c"), delta("d"),
      { type: "message.part.updated", properties: { part: { id: "p1", type: "tool" } } },
      delta("e"),
      { type: "session.error", properties: { sessionID: "s1", error: { name: "UnknownError" } } },
      { type: "session.idle", properties: { sessionID: "s1" } },
      { type: "question.asked", properties: { sessionID: "s1", id: "question1" } },
      { type: "question.replied", properties: { sessionID: "s1", requestID: "question1" } },
      { type: "future.control", properties: {} },
    ]
    const stream = source((index, controller) => {
      if (index) return controller.close()
      controller.enqueue(encoder.encode(payloads.map((payload) => frame(payload, "C:/one")).join("")))
    })
    const beforeControls: string[] = []
    try {
      await streamEvents({ url: "http://engine.test" }, stream.abort.signal, (event, directory) => {
        if ((event.type as string) !== "message.part.delta") beforeControls.push(observer.seen.at(-1)!.text)
        observer.onEvent(event, directory)
      })
      expect(observer.calls).toEqual(payloads.map((payload) => ({ payload, directory: "C:/one" })))
      expect(observer.seen).toEqual([
        { text: "ab", control: "initial" },
        { text: "ab", control: "busy" },
        { text: "ab", control: "idle" },
        { text: "ab", control: "permission.asked" },
        { text: "ab", control: "permission.replied" },
        { text: "abcd", control: "permission.replied" },
        { text: "abcd", control: "message.part.updated" },
        { text: "abcde", control: "message.part.updated" },
        { text: "abcde", control: "session.error" },
        { text: "abcde", control: "session.idle" },
        { text: "abcde", control: "question.asked" },
        { text: "abcde", control: "question.replied" },
        { text: "abcde", control: "future.control" },
      ])
      expect(beforeControls).toEqual(["ab", "ab", "ab", "ab", "abcd", "abcde", "abcde", "abcde", "abcde", "abcde"])
      expect(observer.memoRuns).toBe(3)
      console.log(`Lifecycle reruns: ${observer.memoRuns} delta bursts + ${beforeControls.length} separate controls = ${observer.seen.length} effects`)
    } finally {
      observer.dispose()
    }
  })

  test("partial lines and split UTF-8 do not delay an already complete delta", async () => {
    const observer = observe()
    const partial = encoder.encode(frame(delta("\u00e9\ud83d\ude80"), "C:/\u6587"))
    const split = partial.indexOf(0xf0) + 2
    const chunks = [
      new Uint8Array([...encoder.encode(frame(delta("first"))), ...partial.slice(0, split)]),
      partial.slice(split, partial.length - 4), // Leave CRLF and the blank separator for the next read.
      new Uint8Array([...partial.slice(-4), ...encoder.encode(frame(delta("last"), "C:/two"))]),
    ]
    const stream = source((index, controller) => {
      if (index > 0) {
        expect(observer.text()).toBe(index < 3 ? "first" : "first\u00e9\ud83d\ude80last")
        expect(observer.seen).toHaveLength(index < 3 ? 1 : 2)
      }
      if (index === chunks.length) return controller.close()
      controller.enqueue(chunks[index])
    })
    try {
      await streamEvents({ url: "http://engine.test" }, stream.abort.signal, observer.onEvent)
      expect(observer.calls).toEqual([
        { payload: delta("first"), directory: undefined },
        { payload: delta("\u00e9\ud83d\ude80"), directory: "C:/\u6587" },
        { payload: delta("last"), directory: "C:/two" },
      ])
      expect(observer.memoRuns).toBe(2)
    } finally {
      observer.dispose()
    }
  })

  test("malformed frames and heartbeat noise stay ignored and rearm the existing watchdog", async () => {
    const observer = observe()
    const noise = `: keepalive\n\nevent: message\ndata: {bad json}\ndata: null\ndata: {}\n${frame({ type: "sync" })}${frame({ type: "server.heartbeat" })}`
    let previousTimer: number | undefined
    const stream = source((index, controller) => {
      expect(timers.size).toBe(1)
      const currentTimer = [...timers.keys()][0]
      expect(currentTimer).not.toBe(previousTimer)
      previousTimer = currentTimer
      if (index === 0) return controller.enqueue(encoder.encode(noise))
      if (index === 1) {
        expect(observer.calls).toHaveLength(0)
        return controller.enqueue(encoder.encode(frame(delta("a")) + noise + frame(delta("b"))))
      }
      expect(observer.seen).toEqual([{ text: "ab", control: "initial" }])
      controller.close()
    })
    try {
      await streamEvents({ url: "http://engine.test" }, stream.abort.signal, observer.onEvent)
      expect(observer.calls.map((event) => event.payload)).toEqual([delta("a"), delta("b")])
      expect(stream.fetchSignal?.aborted).toBe(true)
    } finally {
      observer.dispose()
    }
  })

  test.each(["abort", "inactive", "reader error", "callback error"])("%s cleans up without holding the first burst", async (ending) => {
    const observer = observe()
    const stream = source((index, controller) => {
      if (index === 0) return controller.enqueue(encoder.encode(frame(delta("a")) + frame(delta("b"))))
      expect(observer.calls).toHaveLength(2)
      expect(observer.seen).toEqual([{ text: "ab", control: "initial" }])
      if (ending === "abort") stream.abort.abort("test abort")
      if (ending === "inactive") [...timers.values()][0]()
      if (ending === "reader error") controller.error(new Error("reader error"))
    })
    const added = spyOn(stream.abort.signal, "addEventListener")
    const removed = spyOn(stream.abort.signal, "removeEventListener")
    try {
      const pump = streamEvents({ url: "http://engine.test" }, stream.abort.signal, (event, directory) => {
        observer.onEvent(event, directory)
        if (ending === "callback error") throw new Error("callback error")
      })
      if (ending === "abort") await pump
      else await expect(pump).rejects.toThrow(ending)
      expect(stream.fetchSignal?.aborted).toBe(true)
      expect(stream.cancellations).toBe(ending === "reader error" ? 0 : 1)
      expect(timers.size).toBe(0)
      expect(added).toHaveBeenCalledWith("abort", expect.any(Function), { once: true })
      expect(removed).toHaveBeenCalledWith("abort", added.mock.calls[0][1])
      if (ending === "callback error") expect(observer.calls).toHaveLength(1)
    } finally {
      added.mockRestore()
      removed.mockRestore()
      observer.dispose()
    }
  })
}
