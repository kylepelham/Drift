import { expect, test } from "bun:test"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

const { ORCHESTRATOR_AGENT, orchestratorNotice, parseOrchestratorStatus } = await import("../src/state/orchestrator")

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
  // The block must close the reply; trailing prose means the model did not follow the protocol.
  expect(parseOrchestratorStatus(`${block('{"state":"done"}')}\nand one more thing`)).toBeUndefined()
  expect(parseOrchestratorStatus(`${block('{"state":"done"}')}\n  \n`)).toEqual({ state: "done" })
  expect(parseOrchestratorStatus(block('{"state":"finished"}'))).toBeUndefined()
  expect(parseOrchestratorStatus(block("not json"))).toBeUndefined()
  expect(parseOrchestratorStatus("no block at all")).toBeUndefined()
  expect(parseOrchestratorStatus("")).toBeUndefined()
  expect(parseOrchestratorStatus(undefined)).toBeUndefined()
})


const clean = {
  previousStatus: "busy",
  status: "idle",
  agent: ORCHESTRATOR_AGENT,
  parentID: undefined,
  lastMessage: { role: "assistant", completed: true, errored: false, text: block('{"state":"done","headline":"all green"}') },
}

test("a driven turn's ending becomes one notice, and only for a clean orchestrator turn", () => {
  expect(orchestratorNotice(clean)).toEqual({ title: "Orchestrator finished", message: "all green", variant: "success" })
  const said = (text: string) => orchestratorNotice({ ...clean, lastMessage: { ...clean.lastMessage, text } })
  expect(said(block('{"state":"blocked"}'))?.title).toBe("Orchestrator blocked")
  // The engine only ends a clean turn that still says working at its round limit.
  expect(said(block('{"state":"working"}'))?.title).toBe("Orchestrator paused")
  expect(said("no block")?.title).toBe("Orchestrator paused")
  expect(orchestratorNotice({ ...clean, previousStatus: "retry" })).not.toBeNull()
  expect(orchestratorNotice({ ...clean, agent: "build" })).toBeNull()
  expect(orchestratorNotice({ ...clean, parentID: "parent" })).toBeNull()
  expect(orchestratorNotice({ ...clean, status: "busy" })).toBeNull()
  expect(orchestratorNotice({ ...clean, previousStatus: "idle" })).toBeNull()
  expect(orchestratorNotice({ ...clean, lastMessage: undefined })).toBeNull()
  expect(orchestratorNotice({ ...clean, lastMessage: { ...clean.lastMessage, errored: true } })).toBeNull()
  expect(orchestratorNotice({ ...clean, lastMessage: { ...clean.lastMessage, completed: false } })).toBeNull()
})

test("the engine drives the orchestrator; the app only reports how a turn ended", async () => {
  const app = await Bun.file("src/app.tsx").text()
  expect(app).toContain("<OrchestratorBinding />")
  expect(app).not.toContain("actions.steer(")
  const drive = await Bun.file("crates/drift-engine/src/session/drive.rs").text()
  expect(drive).toContain("Proceed toward the goal")
})

test("nudges show as Drift's own prompts, not the user's", async () => {
  const { adaptPart } = await import("../src/engine/native/adapt")
  const part = adaptPart({ id: "p", sessionId: "s", messageId: "m", type: "nudge", text: "Proceed toward the goal." } as never)
  expect(part).toMatchObject({ type: "text", text: "Proceed toward the goal.", metadata: { generated: true } })
})

test("async questions do not mark tools as awaiting permission", async () => {
  const parts = await Bun.file("src/ui/parts.tsx").text()
  expect(parts).toContain("(question) => !question.async && question.tool?.callID === part.callID")
})

test("the orchestrator agent is a native built-in with delegation-only tools and the status protocol", async () => {
  const builtins = await Bun.file("crates/drift-engine/src/config/mod.rs").text()
  const defined = builtins.split("\n").find((line) => line.includes(`agent("${ORCHESTRATOR_AGENT}",`))
  expect(defined).toBeDefined()
  expect(defined).toContain("AgentKind::Primary")
  // An allowlist without the implementation tools, so all substantial work flows through subagents.
  for (const tool of ['"edit"', '"write"', '"apply_patch"', '"bash"']) expect(defined).not.toContain(tool)
  expect(defined).toContain('"task"')
  const prompt = await Bun.file("crates/drift-engine/src/config/prompts/orchestrator.txt").text()
  expect(prompt).toContain("<orchestrator_status>")
  expect(prompt).toContain('"working"')
  expect(prompt).toContain("Never ask the user whether to continue")
  expect(prompt).toContain("Never claim done without verification evidence")
})
