import { expect, test } from "bun:test"
import type { ToolPart } from "@opencode-ai/sdk/client"
import { createActions } from "../src/engine/actions"
import type { Client, TaskRecord } from "../src/engine/native/client"
import { createEngineState, mergeTasks, putTasks } from "../src/engine/store"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", { value: { getItem: () => null, setItem: () => undefined } })

function task(id: string, overrides: Partial<TaskRecord> = {}): TaskRecord {
  return {
    id,
    parentSessionId: "parent",
    sessionId: `worker_${id}`,
    callId: `call_${id}`,
    description: `do ${id}`,
    agent: "explore",
    mode: "background",
    reason: "requested",
    state: "running",
    delivered: false,
    held: false,
    createdAt: 1,
    ...overrides,
  }
}

// What the native engine's `task` call looks like once a background launch has returned its receipt.
function receipt(id: string): ToolPart {
  return {
    id: `part_${id}`,
    type: "tool",
    tool: "task",
    sessionID: "parent",
    messageID: "message",
    callID: `call_${id}`,
    state: {
      status: "completed",
      input: { description: `do ${id}`, subagent_type: "explore", run_in_background: true },
      output: `Started do ${id} in the background as ${id} (@explore).`,
      title: `do ${id}`,
      metadata: { sessionId: `worker_${id}`, taskId: id, outcome: "launched", mode: "background" },
      time: { start: 1, end: 2 },
    },
  }
}

test("a snapshot that raced an event never moves a task back", () => {
  const live = [task("a", { state: "replied", delivered: true, finishedAt: 5 })]
  const stale = [task("a", { state: "running" }), task("b", { createdAt: 0 })]
  const merged = mergeTasks(live, stale)
  expect(merged.map((t) => [t.id, t.state, t.delivered])).toEqual([
    ["b", "running", false],
    ["a", "replied", true],
  ])
  expect(mergeTasks(merged, [task("a", { state: "replied", delivered: true, result: "later copy" })])[1]!.result).toBe("later copy")
  // Held after a Stop is past ended, and carried by the next prompt is past held.
  const held = mergeTasks([task("h", { state: "stopped", held: true })], [task("h", { state: "stopped" })])
  expect(held[0]!.held).toBe(true)
  expect(mergeTasks(held, [task("h", { state: "stopped", held: true, delivered: true })])[0]!.delivered).toBe(true)
})

test("a background task row follows its worker, not the call that launched it", async () => {
  const { delegatedTaskStatus } = await import("../src/ui/parts")
  const [state, set] = createEngineState()
  const part = receipt("a")
  // The launch receipt is a finished call, but it is not the worker finishing.
  expect(delegatedTaskStatus(state, part, "worker_a")).toBe("running")
  putTasks(set, state, "parent", [task("a", { state: "queued" })])
  expect(delegatedTaskStatus(state, part, "worker_a")).toBe("running")
  putTasks(set, state, "parent", [task("a", { state: "running" })])
  expect(delegatedTaskStatus(state, part, "worker_a")).toBe("running")
  putTasks(set, state, "parent", [task("a", { state: "replied", finishedAt: 3 })])
  expect(delegatedTaskStatus(state, part, "worker_a")).toBe("completed")
  for (const ended of ["failed", "stopped", "interrupted"] as const) {
    const [other, setOther] = createEngineState()
    putTasks(setOther, other, "parent", [task("a", { state: ended })])
    expect(delegatedTaskStatus(other, part, "worker_a")).toBe("error")
  }
})

test("a running foreground call is matched to its task by call id before its metadata lands", async () => {
  const { delegatedTaskStatus } = await import("../src/ui/parts")
  const [state, set] = createEngineState()
  const running = { ...receipt("f"), state: { status: "running", input: {}, time: { start: 1 } } } as ToolPart
  putTasks(set, state, "parent", [task("f", { mode: "foreground", state: "running" })])
  expect(delegatedTaskStatus(state, running, "worker_f")).toBe("running")
})

test("the dock lists background workers while any is going or owed, and never foreground ones", async () => {
  const { dockTasks } = await import("../src/ui/task-dock")
  const foreground = task("f", { mode: "foreground", state: "running" })
  expect(dockTasks(undefined)).toEqual([])
  expect(dockTasks([foreground])).toEqual([])
  const done = task("a", { state: "replied", delivered: true })
  const going = task("b", { state: "running" })
  expect(dockTasks([foreground, done, going]).map((t) => t.id)).toEqual(["a", "b"])
  // Finished but not yet handed to the conversation still counts as outstanding.
  expect(dockTasks([done, task("c", { state: "failed", delivered: false })]).map((t) => t.id)).toEqual(["a", "c"])
  expect(dockTasks([done, task("c", { state: "stopped", delivered: true })])).toEqual([])
  // A result held back by Stop stays listed until a prompt carries it.
  expect(dockTasks([done, task("c", { state: "replied", held: true })]).map((t) => t.id)).toEqual(["a", "c"])
})

function harness(overrides: Partial<Client>) {
  const client = {
    messages: () => Promise.resolve([]),
    todos: () => Promise.resolve([]),
    ...overrides,
  } as unknown as Client
  const [state, set] = createEngineState()
  const workspaces = () => ({ path: () => undefined, id: () => undefined })
  return { state, actions: createActions(() => client, state, set, workspaces) }
}

test("opening a conversation loads the workers it launched", async () => {
  const asked: string[] = []
  const h = harness({ tasks: (id: string) => (asked.push(id), Promise.resolve([task("a")])) } as Partial<Client>)
  expect(await h.actions.openSession("parent")).toBe(true)
  expect(asked).toEqual(["parent"])
  expect(h.state.tasks.parent!.map((t) => t.id)).toEqual(["a"])
})

test("a conversation still opens when its task list cannot be read", async () => {
  const h = harness({ tasks: () => Promise.reject(new Error("offline")) } as Partial<Client>)
  expect(await h.actions.openSession("parent")).toBe(true)
  expect(h.state.tasks.parent).toBeUndefined()
})

test("stopping a task stops that task and records what the engine says", async () => {
  const stopped: string[] = []
  const h = harness({ stopTask: (id: string) => (stopped.push(id), Promise.resolve(task(id, { state: "stopped", finishedAt: 4 }))) } as Partial<Client>)
  await h.actions.stopTask("a")
  expect(stopped).toEqual(["a"])
  expect(h.state.tasks.parent![0]!.state).toBe("stopped")
})

test("a stop the engine refuses says so instead of failing silently", async () => {
  const h = harness({ stopTask: () => Promise.reject(new Error("no task")) } as Partial<Client>)
  await h.actions.stopTask("gone")
  expect(h.state.notices.at(-1)).toMatchObject({ id: "task-stop-gone", variant: "error", message: "no task" })
})
