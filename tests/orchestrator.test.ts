import { expect, test } from "bun:test"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

const {
  isGeneratedUserEntry,
  ORCHESTRATOR_AGENT,
  orchestratorGate,
  orchestratorMaxRounds,
  parseOrchestratorStatus,
  PROCEED_PROMPT,
  STATUS_REMINDER_PROMPT,
} = await import("../src/state/orchestrator")

const block = (body: string) => `<orchestrator_status>\n${body}\n</orchestrator_status>`

test("status parsing is strict, takes the last block, and fails closed on anything else", () => {
  expect(parseOrchestratorStatus(`dispatched two tasks\n${block('{"state":"working","headline":"reviewing results"}')}`))
    .toEqual({ state: "working", headline: "reviewing results" })
  expect(parseOrchestratorStatus(block('{"state":"done"}'))).toEqual({ state: "done" })
  expect(parseOrchestratorStatus(block('{"state":"blocked","headline":"  need the API key  "}'))).toEqual({
    state: "blocked",
    headline: "need the API key",
  })
  // The final block wins when the model quotes an earlier one.
  expect(
    parseOrchestratorStatus(
      `${block('{"state":"working"}')}\nmore text\n${block('{"state":"done","headline":"all tests pass"}')}`,
    ),
  ).toEqual({ state: "done", headline: "all tests pass" })
  expect(parseOrchestratorStatus(block('{"state":"finished"}'))).toBeUndefined()
  expect(parseOrchestratorStatus(block("not json"))).toBeUndefined()
  expect(parseOrchestratorStatus("no block at all")).toBeUndefined()
  expect(parseOrchestratorStatus("")).toBeUndefined()
  expect(parseOrchestratorStatus(undefined)).toBeUndefined()
})

const eligible = {
  previousStatus: "busy",
  status: "idle",
  goalAgent: ORCHESTRATOR_AGENT,
  parentID: undefined,
  pendingAsks: 0,
  lastMessage: { role: "assistant", completed: true, errored: false },
  rounds: 0,
}

test("the driver only acts on clean turn completions of orchestrator sessions", () => {
  expect(orchestratorGate(eligible)).toBeNull()
  expect(orchestratorGate({ ...eligible, previousStatus: "retry" })).toBeNull()
  expect(orchestratorGate({ ...eligible, goalAgent: "build" })).toBe("not an orchestrator session")
  expect(orchestratorGate({ ...eligible, goalAgent: undefined })).toBe("not an orchestrator session")
  expect(orchestratorGate({ ...eligible, status: "busy" })).toBe("not idle")
  expect(orchestratorGate({ ...eligible, previousStatus: "idle" })).toBe("not a turn completion")
  expect(orchestratorGate({ ...eligible, parentID: "parent" })).toBe("subagent")
  expect(orchestratorGate({ ...eligible, pendingAsks: 1 })).toBe("awaiting permission or question")
  expect(orchestratorGate({ ...eligible, lastMessage: undefined })).toBe("no final message")
  expect(orchestratorGate({ ...eligible, lastMessage: { role: "user", completed: true, errored: false } })).toBe(
    "no assistant reply",
  )
  expect(orchestratorGate({ ...eligible, lastMessage: { role: "assistant", completed: false, errored: false } })).toBe(
    "reply not completed",
  )
  expect(orchestratorGate({ ...eligible, lastMessage: { role: "assistant", completed: true, errored: true } })).toBe(
    "reply errored",
  )
  expect(orchestratorGate({ ...eligible, rounds: orchestratorMaxRounds })).toBe("round limit reached")
})

test("driver prompts push forward without re-summarizing, and enforce the protocol", () => {
  expect(PROCEED_PROMPT).toContain("Proceed toward the goal")
  expect(PROCEED_PROMPT).toContain("Do not re-summarize completed work")
  expect(STATUS_REMINDER_PROMPT).toContain("<orchestrator_status>")
  expect(STATUS_REMINDER_PROMPT).toContain("Proceed toward the goal")
})

test("generated steering prompts never count as a fresh goal", () => {
  expect(isGeneratedUserEntry([{ type: "text", metadata: { generated: true } }])).toBeTrue()
  expect(isGeneratedUserEntry([{ type: "text" }])).toBeFalse()
  expect(isGeneratedUserEntry([{ type: "text", metadata: { generated: true } }, { type: "text" }])).toBeFalse()
  expect(isGeneratedUserEntry([{ type: "file" }])).toBeFalse()
})

test("steer sends a generated prompt through the session's own agent", async () => {
  const { createActions } = await import("../src/engine/actions")
  const { createEngineState } = await import("../src/engine/store")
  const [state, set] = createEngineState()
  let body: unknown
  const client = {
    session: {
      promptAsync: async (input: { body: unknown }) => {
        body = input.body
        return { data: {} }
      },
    },
  }
  const actions = createActions(() => client as never, state, set, () => undefined)
  expect(await actions.steer("ses", PROCEED_PROMPT, { model: null, agent: ORCHESTRATOR_AGENT })).toEqual({ ok: true })
  expect(body).toMatchObject({
    parts: [{ type: "text", text: PROCEED_PROMPT, metadata: { generated: true } }],
    agent: ORCHESTRATOR_AGENT,
  })

  const failing = {
    session: { promptAsync: async () => ({ error: { data: { message: "engine rejected the request" } } }) },
  }
  const failingActions = createActions(() => failing as never, state, set, () => undefined)
  expect(await failingActions.steer("ses", PROCEED_PROMPT, { model: null, agent: ORCHESTRATOR_AGENT })).toMatchObject({
    ok: false,
  })
})

test("the orchestrator agent is defined with delegation-only tools and the status protocol", async () => {
  const config = JSON.parse(await Bun.file("engine/opencode/opencode.json").text()) as {
    agent?: Record<string, { mode?: string; prompt?: string; tools?: Record<string, boolean> }>
  }
  const agent = config.agent?.[ORCHESTRATOR_AGENT]
  expect(agent).toBeDefined()
  expect(agent!.mode).toBe("primary")
  // Implementation tools are denied so all substantial work flows through subagents.
  expect(agent!.tools).toMatchObject({ edit: false, write: false, apply_patch: false, bash: false })
  expect(agent!.prompt).toContain("<orchestrator_status>")
  expect(agent!.prompt).toContain('"working"')
  expect(agent!.prompt).toContain("Never ask the user whether to continue")
  expect(agent!.prompt).toContain("Never claim done without verification evidence")
})

test("the driver is wired into the app and reacts to status transitions", async () => {
  const app = await Bun.file("src/app.tsx").text()
  expect(app).toContain("<OrchestratorBinding />")
  // Driving escapes the status effect's tracking scope.
  expect(app).toContain("queueMicrotask(() => void drive(id, before))")
  // done and blocked stop the loop with a user-visible notice instead of another prompt.
  expect(app).toMatch(/state === "done"[\s\S]*?variant: "success"/)
  expect(app).toMatch(/state === "blocked"[\s\S]*?variant: "warning"/)
})
