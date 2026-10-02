import { afterEach, expect, test } from "bun:test"
import { connectEvents } from "../src/engine/native/events"
import type { Envelope, Frame } from "../src/engine/native/client"

type Socket = { send(text: string): void; close(): void }

/** A stand-in engine: records connect cursors and lets tests push frames to the latest socket. */
function fakeEngine() {
  const cursors: (string | null)[] = []
  const sockets: Socket[] = []
  const server = Bun.serve<{ cursor: string | null }, {}>({
    port: 0,
    fetch(request, server) {
      const url = new URL(request.url)
      if (server.upgrade(request, { data: { cursor: url.searchParams.get("cursor") } })) return
      return new Response("expected websocket", { status: 400 })
    },
    websocket: {
      open(ws) {
        cursors.push(ws.data.cursor)
        sockets.push(ws)
      },
      message() {},
    },
  })
  return {
    target: { url: `http://127.0.0.1:${server.port}`, token: "t" },
    cursors,
    latest: () => sockets[sockets.length - 1],
    send: (frame: Frame) => sockets[sockets.length - 1].send(JSON.stringify(frame)),
    hello: (seq: number, instance = "one") => sockets[sockets.length - 1].send(JSON.stringify({ type: "hello", version: "0", instance, seq })),
    stop: () => server.stop(true),
  }
}

function workspaceEvent(seq: number): Envelope {
  return { seq, type: "workspace.created", workspace: { id: `w${seq}`, path: "C:/w", name: "w", icon: "", lastUsed: 0 } }
}

const until = (predicate: () => boolean) =>
  new Promise<void>((resolve, reject) => {
    const started = Date.now()
    const tick = () => {
      if (predicate()) return resolve()
      if (Date.now() - started > 2000) return reject(new Error("timed out"))
      setTimeout(tick, 5)
    }
    tick()
  })

let stops: (() => void)[] = []
afterEach(() => {
  for (const stop of stops) stop()
  stops = []
})

test("first hello hydrates and sets the cursor", async () => {
  const engine = fakeEngine()
  const hydrated: number[] = []
  const stream = connectEvents(engine.target, { hydrate: (seq) => void hydrated.push(seq), event: () => {} })
  stops.push(engine.stop, stream.close)
  await until(() => engine.cursors.length === 1)
  engine.hello(5)
  await until(() => hydrated.length === 1)
  expect(hydrated).toEqual([5])
  await until(() => stream.cursor() === 5)
  expect(engine.cursors[0]).toBeNull()
})

test("events advance the cursor and a reconnect resumes from it without hydrating again", async () => {
  const engine = fakeEngine()
  const hydrated: number[] = []
  const seen: number[] = []
  let resumed = 0
  const stream = connectEvents(engine.target, {
    hydrate: (seq) => void hydrated.push(seq),
    event: (envelope) => seen.push(envelope.seq),
    resumed: () => void (resumed += 1),
  })
  stops.push(engine.stop, stream.close)
  await until(() => engine.cursors.length === 1)
  engine.hello(0)
  await until(() => stream.cursor() === 0)
  engine.send(workspaceEvent(1))
  engine.send(workspaceEvent(2))
  await until(() => seen.length === 2)
  expect(stream.cursor()).toBe(2)

  engine.latest().close()
  await until(() => engine.cursors.length === 2)
  expect(engine.cursors[1]).toBe("2")
  engine.hello(3)
  engine.send(workspaceEvent(3))
  await until(() => seen.length === 3)
  expect(hydrated).toEqual([0])
  expect(resumed, "the resume is reported, so the UI comes back online without a hydrate").toBe(1)
})

test("a hello from a different engine instance hydrates again", async () => {
  const engine = fakeEngine()
  const hydrated: number[] = []
  const stream = connectEvents(engine.target, { hydrate: (seq) => void hydrated.push(seq), event: () => {} })
  stops.push(engine.stop, stream.close)
  await until(() => engine.cursors.length === 1)
  engine.hello(4, "first")
  await until(() => stream.cursor() === 4)
  engine.latest().close()
  await until(() => engine.cursors.length === 2)
  engine.hello(1, "second")
  await until(() => hydrated.length === 2)
  expect(hydrated).toEqual([4, 1])
  await until(() => stream.cursor() === 1)
})

test("resync hydrates again from the given seq", async () => {
  const engine = fakeEngine()
  const hydrated: number[] = []
  const stream = connectEvents(engine.target, { hydrate: (seq) => void hydrated.push(seq), event: () => {} })
  stops.push(engine.stop, stream.close)
  await until(() => engine.cursors.length === 1)
  engine.hello(1)
  engine.send({ type: "resync", seq: 9 })
  await until(() => hydrated.length === 2)
  expect(hydrated).toEqual([1, 9])
  await until(() => stream.cursor() === 9)
})

test("events that arrive during hydration wait for it and skip anything the snapshot covered", async () => {
  const engine = fakeEngine()
  const seen: number[] = []
  let finish!: () => void
  const stream = connectEvents(engine.target, {
    hydrate: () => new Promise<void>((resolve) => (finish = resolve)),
    event: (envelope) => seen.push(envelope.seq),
  })
  stops.push(engine.stop, stream.close)
  await until(() => engine.cursors.length === 1)
  engine.hello(2)
  await until(() => Boolean(finish))
  engine.send(workspaceEvent(2))
  engine.send(workspaceEvent(3))
  await new Promise((resolve) => setTimeout(resolve, 50))
  expect(seen).toEqual([])
  finish()
  await until(() => seen.length === 1)
  expect(seen).toEqual([3])
  expect(stream.cursor()).toBe(3)
})

test("close stops reconnecting", async () => {
  const engine = fakeEngine()
  const stream = connectEvents(engine.target, { hydrate: () => {}, event: () => {} })
  stops.push(engine.stop)
  await until(() => engine.cursors.length === 1)
  stream.close()
  await new Promise((resolve) => setTimeout(resolve, 700))
  expect(engine.cursors.length).toBe(1)
})

test("a failed hydrate keeps the cursor and retries, applying held events once it succeeds", async () => {
  const engine = fakeEngine()
  let attempts = 0
  const seen: number[] = []
  const stream = connectEvents(engine.target, {
    hydrate: () => {
      attempts += 1
      if (attempts === 1) throw new Error("engine hiccup")
    },
    event: (envelope) => seen.push(envelope.seq),
  })
  stops.push(engine.stop, stream.close)
  await until(() => engine.cursors.length === 1)
  engine.hello(3)
  engine.send(workspaceEvent(4))
  await until(() => attempts === 1)
  expect(stream.cursor()).toBeUndefined()
  expect(seen).toEqual([])
  await until(() => attempts === 2 && seen.length === 1)
  expect(seen).toEqual([4])
  expect(stream.cursor()).toBe(4)
})

test("a resync during hydration folds into the same run instead of dropping held events", async () => {
  const engine = fakeEngine()
  const hydrated: number[] = []
  const seen: number[] = []
  let finish!: () => void
  const stream = connectEvents(engine.target, {
    hydrate: (seq) => {
      hydrated.push(seq)
      return hydrated.length === 1 ? new Promise<void>((resolve) => (finish = resolve)) : undefined
    },
    event: (envelope) => seen.push(envelope.seq),
  })
  stops.push(engine.stop, stream.close)
  await until(() => engine.cursors.length === 1)
  engine.hello(1)
  await until(() => Boolean(finish))
  engine.send(workspaceEvent(2))
  engine.send({ type: "resync", seq: 5 })
  engine.send(workspaceEvent(6))
  await new Promise((resolve) => setTimeout(resolve, 30))
  finish()
  await until(() => seen.length === 1)
  expect(hydrated).toEqual([1, 5])
  expect(seen).toEqual([6])
  expect(stream.cursor()).toBe(6)
})

test("held events from an old engine instance never apply after the instance changes", async () => {
  const engine = fakeEngine()
  const hydrated: number[] = []
  const seen: number[] = []
  const finishers: (() => void)[] = []
  const stream = connectEvents(engine.target, {
    hydrate: (seq) => {
      hydrated.push(seq)
      return new Promise<void>((resolve) => finishers.push(resolve))
    },
    event: (envelope) => seen.push(envelope.seq),
  })
  stops.push(engine.stop, stream.close)
  await until(() => engine.cursors.length === 1)
  engine.hello(1, "first")
  await until(() => finishers.length === 1)
  engine.send(workspaceEvent(2))
  engine.latest().close()
  await until(() => engine.cursors.length === 2)
  engine.hello(0, "second")
  await until(() => finishers.length === 2)
  engine.send(workspaceEvent(1))
  finishers[0]!()
  await new Promise((resolve) => setTimeout(resolve, 30))
  expect(seen).toEqual([])
  expect(stream.cursor()).toBeUndefined()
  finishers[1]!()
  await until(() => seen.length === 1)
  expect(seen).toEqual([1])
  expect(hydrated).toEqual([1, 0])
})

test("closing during hydration drops the held events", async () => {
  const engine = fakeEngine()
  const seen: number[] = []
  let finish!: () => void
  const stream = connectEvents(engine.target, { hydrate: () => new Promise<void>((resolve) => (finish = resolve)), event: (e) => seen.push(e.seq) })
  stops.push(engine.stop)
  await until(() => engine.cursors.length === 1)
  engine.hello(0)
  await until(() => Boolean(finish))
  engine.send(workspaceEvent(1))
  stream.close()
  finish()
  await new Promise((resolve) => setTimeout(resolve, 30))
  expect(seen).toEqual([])
})
