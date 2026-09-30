import { afterAll, afterEach, beforeAll, expect, test } from "bun:test"
import { readFileSync, writeFileSync } from "node:fs"
import path from "node:path"
import { fakeAnthropic, fixture, model, startEngine, type Engine } from "./harness"

const root = path.resolve(import.meta.dir, "../..")
let fake: ReturnType<typeof fakeAnthropic>
let engine: Engine

beforeAll(async () => {
  const build = Bun.spawnSync(["cargo", "build", "-q", "-p", "drift-engined"], { cwd: root, stdout: "inherit", stderr: "inherit" })
  if (build.exitCode !== 0) throw new Error("drift-engined did not build")
  fake = fakeAnthropic()
  engine = await startEngine(fake.url)
}, 300_000)

afterAll(async () => {
  await engine.stop()
  engine.cleanup()
  fake.stop()
}, 30_000)
afterEach(() => {
  fake.seen.length = 0
}, 30_000)
const submit = (session: string, text: string, extra: Record<string, unknown> = {}) =>
  engine.call<Record<string, unknown>>("POST", `/sessions/${session}/turns`, { parts: [{ type: "text", text }], model, ...extra })

test("a recorded tool turn streams over the socket, asks for permission, and replays through the real adapter", async () => {
  const session = await engine.setup()
  writeFileSync(path.join(engine.workspace, "hello.txt"), "hello\n")
  const events = engine.events()
  await events.opened
  // The recorded path arrives split across two deltas; swap both halves so the call lands on hello.txt.
  fake.push({ body: fixture("tool_call").replace("src/ma", "hello.t").replace("in.rs", "xt") }, { body: fixture("text") })

  const accepted = await submit(session, "what is in hello.txt?")
  expect(accepted.status).toBe(202)
  await events.until((f) => f.type === "session.status" && f.status === "running")
  const delta = await events.until((f) => f.type === "part.delta")
  expect(typeof delta.delta).toBe("string")
  const done = await events.until((f) => f.type === "part.updated" && (f.part as { type: string; status: string }).type === "tool_call" && (f.part as { status: string }).status === "done")
  expect((done.part as { output: string }).output).toBe("1: hello")
  await events.until((f) => f.type === "session.status" && f.status === "idle")

  expect(fake.seen).toHaveLength(2)
  const first = fake.seen[0]!
  expect(first.headers["x-api-key"]).toBe("sk-conformance")
  expect(first.headers["anthropic-version"]).toBe("2023-06-01")
  expect((first.body.system as { cache_control: unknown }[])[0]!.cache_control).toEqual({ type: "ephemeral" })
  const second = fake.seen[1]!.body.messages as { role: string; content: { type: string; signature?: string; tool_use_id?: string }[] }[]
  const assistant = second.find((m) => m.role === "assistant")!
  expect(assistant.content[0]!.type).toBe("thinking")
  expect(assistant.content[0]!.signature).toBe("EqQBCgIYAhIM")
  expect(second.at(-1)!.content[0]!.tool_use_id).toBe("toolu_01A")

  const messages = await engine.call<{ role: string; status: string; parts: { type: string; text?: string }[]; usage: { cacheRead: number } }[]>("GET", `/sessions/${session}/messages`)
  expect(messages.json.map((m) => m.status)).toEqual(["done", "done", "done"])
  expect(messages.json[2]!.parts[0]!.text).toBe("The file says hello.")
  expect(messages.json[2]!.usage.cacheRead).toBe(30)
  events.close()
}, 30_000)

test("a stream that ends without a stop reason fails the message and runs nothing", async () => {
  const session = await engine.setup()
  const events = engine.events()
  await events.opened
  fake.push({ body: fixture("truncated") }, { body: fixture("truncated") }, { body: fixture("text") })
  await submit(session, "write never.txt")
  await events.until((f) => f.type === "session.status" && f.status === "idle", 20_000)
  const messages = await engine.call<{ role: string; status: string; error?: string; parts: { status?: string }[] }[]>("GET", `/sessions/${session}/messages`)
  const attempts = messages.json.filter((m) => m.role === "assistant")
  expect(attempts.map((m) => m.status)).toEqual(["error", "error", "done"])
  for (const attempt of attempts.slice(0, 2)) {
    expect(attempt.status).toBe("error")
    expect(attempt.error).toContain("stop reason")
    expect(attempt.parts[0]!.status).toBe("pending")
  }
  expect(() => readFileSync(path.join(engine.workspace, "never.txt"))).toThrow()
  events.close()
}, 30_000)

test("a denied permission reaches the model as an error result and never touches the disk", async () => {
  const session = await engine.setup()
  const events = engine.events()
  await events.opened
  const write = fixture("tool_call").replace('"name":"read"', '"name":"write"').replace('{\\"path\\": \\"src/ma', '{\\"path\\": \\"denied.txt\\", \\"content\\": \\"x\\"').replace('in.rs\\"}', "}")
  fake.push({ body: write }, { body: fixture("text") })
  await submit(session, "write denied.txt")
  const asked = await events.until((f) => f.type === "permission.asked")
  const request = asked.request as { id: string; tool: string; kind: string }
  expect(request.tool).toBe("write")
  expect(request.kind).toBe("edit")
  const pending = await engine.call<{ id: string }[]>("GET", "/permissions")
  expect(pending.json[0]!.id).toBe(request.id)
  await engine.call("POST", `/permissions/${request.id}/reply`, { reply: "deny" })
  await events.until((f) => f.type === "session.status" && f.status === "idle")
  const result = (fake.seen[1]!.body.messages as { content: { type: string; is_error?: boolean; content?: string }[] }[]).at(-1)!.content[0]!
  expect(result.type).toBe("tool_result")
  expect(result.is_error).toBe(true)
  expect(result.content).toContain("denied")
  expect(() => readFileSync(path.join(engine.workspace, "denied.txt"))).toThrow()
  events.close()
}, 30_000)

test("overloaded responses are retried and a resubmitted id is one prompt", async () => {
  const session = await engine.setup()
  const events = engine.events()
  await events.opened
  fake.push({ status: 529, body: JSON.stringify({ error: { type: "overloaded_error", message: "busy" } }) }, { body: fixture("text") })
  const first = await submit(session, "hi", { submissionId: "conf_1" })
  const again = await submit(session, "hi", { submissionId: "conf_1" })
  expect((again.json.message as { id: string }).id).toBe((first.json.message as { id: string }).id)
  const changed = await submit(session, "different", { submissionId: "conf_1" })
  expect(changed.status).toBe(409)
  await events.until((f) => f.type === "session.status" && f.status === "idle", 20_000)
  expect(fake.seen).toHaveLength(2)
  const messages = await engine.call<{ role: string; status: string }[]>("GET", `/sessions/${session}/messages`)
  expect(messages.json.map((m) => `${m.role}:${m.status}`)).toEqual(["user:done", "assistant:error", "assistant:done"])
  events.close()
}, 30_000)

test("abort during a slow stream marks the message aborted and frees the session", async () => {
  const session = await engine.setup()
  const events = engine.events()
  await events.opened
  fake.push({ body: fixture("text"), delayMs: 3_000 })
  await submit(session, "slow")
  await events.until((f) => f.type === "session.status" && f.status === "running")
  const busy = await submit(session, "again")
  expect(busy.status).toBe(409)
  const aborted = await engine.call<{ aborted: boolean }>("POST", `/sessions/${session}/abort`)
  expect(aborted.json.aborted).toBe(true)
  await events.until((f) => f.type === "session.status" && f.status === "idle")
  const messages = await engine.call<{ status: string }[]>("GET", `/sessions/${session}/messages`)
  expect(messages.json.at(-1)!.status).toBe("aborted")
  events.close()
}, 30_000)

test("reconnecting with a cursor replays what was missed; a restarted engine announces a new instance", async () => {
  const session = await engine.setup()
  const first = engine.events()
  await first.opened
  const hello = await first.until((f) => f.type === "hello")
  const instance = hello.instance
  fake.push({ body: fixture("text") })
  await submit(session, "one")
  const idle = await first.until((f) => f.type === "session.status" && f.status === "idle")
  const cursor = (idle.seq as number) - 2
  first.close()

  const resumed = engine.events(cursor)
  await resumed.opened
  const replayed = await resumed.until((f) => f.type === "session.status" && f.status === "idle")
  expect(replayed.seq).toBe(idle.seq)
  expect(resumed.frames.filter((f) => f.seq !== undefined).every((f) => (f.seq as number) > cursor)).toBe(true)
  resumed.close()

  const dataDir = engine.dataDir
  await engine.stop()
  engine = await startEngine(fake.url, dataDir)
  const fresh = engine.events(idle.seq as number)
  await fresh.opened
  const again = await fresh.until((f) => f.type === "hello")
  expect(again.instance).not.toBe(instance)
  await fresh.until((f) => f.type === "resync")
  const kept = await engine.call<{ id: string }>("GET", `/sessions/${session}`)
  expect(kept.status).toBe(200)
  fresh.close()
}, 30_000)