import { agentBehaviorIssue, agentOverrideValue, applicableOverride } from "../src/state/prompts"
import { agentModelCapability, agentModelOptions } from "../src/state/agent-models"
import { hiddenModelIds, setHiddenModelIds } from "../src/state/prefs"
import { expect, test } from "bun:test"

import type { ModelInfo, ProviderInfo } from "../src/engine/store"

function model(id: string, name = id, toolcall = true, context = 65536, text = true): ModelInfo {
  return {
    id,
    name,
    capabilities: { toolcall, input: { text }, output: { text } },
    limit: { context },
  } as ModelInfo
}

function provider(id: string, models: ModelInfo[]): ProviderInfo {
  return { id, name: id.toUpperCase(), models: Object.fromEntries(models.map((item) => [item.id, item])) }
}

test("model selection is available for subagents and active utility agents", () => {
  expect(agentModelCapability({ name: "general", mode: "subagent" })).toBe("tools")
  expect(agentModelCapability({ name: "custom", mode: "all" })).toBe("tools")
  expect(agentModelCapability({ name: "title", mode: "primary" })).toBe("text")
  expect(agentModelCapability({ name: "compaction", mode: "primary" })).toBe("text")
  expect(agentModelCapability({ name: "summary", mode: "primary" })).toBeUndefined()
  expect(agentModelCapability({ name: "build", mode: "primary" })).toBeUndefined()
})

test("subagent model choices include hidden tool models and preserve provider-qualified IDs", () => {
  const hidden = hiddenModelIds()
  setHiddenModelIds(["one/cheap"])
  try {
    const providers = [
      provider("one", [model("vendor/review", "Smart"), model("cheap", "Cheap"), model("embed", "Embedding", false)]),
      provider("two", [model("cheap", "Cheap")]),
      provider("offline", [model("unavailable")]),
    ]
    expect(agentModelOptions({ providers, connected: ["one", "two"] })).toEqual([
      { id: "one/cheap", label: "Cheap", group: "ONE", detail: "cheap" },
      { id: "one/vendor/review", label: "Smart", group: "ONE", detail: "vendor/review" },
      { id: "two/cheap", label: "Cheap", group: "TWO", detail: "cheap" },
    ])
  } finally {
    setHiddenModelIds(hidden)
  }
})

test("subagent model choices respect LM Studio context readiness and disconnected providers", () => {
  const providers = [provider("lmstudio", [model("small", "Small", true, 4096), model("ready")])]
  expect(agentModelOptions({ providers, connected: ["lmstudio"] }).map((item) => item.id)).toEqual(["lmstudio/ready"])
  expect(agentModelOptions({ providers, connected: [] })).toEqual([])
})

test("utility agent choices include text models without tool calling and exclude non-text models", () => {
  const providers = [
    provider("one", [
      model("tools", "Tools"),
      model("cheap-text", "Cheap text", false),
      model("embedding", "Embedding", false, 65536, false),
    ]),
  ]
  expect(agentModelOptions({ providers, connected: ["one"] }, "text").map((item) => item.id)).toEqual([
    "one/cheap-text",
    "one/tools",
  ])

  const local = [provider("lmstudio", [model("small", "Small", false, 4096), model("local-text", "Local text", false)])]
  expect(agentModelOptions({ providers: local, connected: ["lmstudio"] }, "text").map((item) => item.id)).toEqual([
    "lmstudio/local-text",
  ])
})

test("picking a model changes only the model and keeps every other field", async () => {
  const { configOf, draftOf } = await import("../src/ui/settings-prompts")
  const baseline = {
    prompt: "Review carefully",
    permissions: [{ kind: "edit", pattern: "*", decision: "deny" }],
    steps: 12,
  }
  const draft = { ...draftOf(baseline), model: "provider/vendor/reviewer" }
  const config = configOf(draft, baseline) as Record<string, unknown>
  expect(config).toEqual({ ...baseline, model: "provider/vendor/reviewer" })
  expect(agentOverrideValue(config, baseline)).toEqual({ model: "provider/vendor/reviewer" })
})

test("Current model is dynamic inheritance and explicitly masks an underlying model pin", async () => {
  const { configOf, draftOf } = await import("../src/ui/settings-prompts")
  expect(draftOf({}).model).toBe("")
  const baseline = { model: "provider/smart", prompt: "Keep prompt" }
  const config = configOf({ ...draftOf(baseline), model: "" }, baseline) as Record<string, unknown>
  expect(config.model, "an emptied pin is sent as empty, not left out").toBe("")
  expect(agentOverrideValue(config, baseline, { prompt: "Keep prompt", model: "provider/smart" })).toEqual({
    prompt: "Keep prompt",
    model: "",
  })
  expect(configOf(draftOf({ prompt: "No pin" }), { prompt: "No pin" })).toEqual({ prompt: "No pin" })
})

test("a saved model no longer offered still shows as the selection", async () => {
  const { draftOf } = await import("../src/ui/settings-prompts")
  expect(draftOf({ model: "removed-provider/old-model" }).model).toBe("removed-provider/old-model")
})

test("tools read as all, only these, or all except these, and go back the same way", async () => {
  const { configOf, draftOf } = await import("../src/ui/settings-prompts")
  expect(draftOf({}).toolMode).toBe("all")
  const only = draftOf({ tools: ["read", "grep"] })
  expect([only.toolMode, only.tools]).toEqual(["only", ["read", "grep"]])
  const except = draftOf({ tools: ["!bash", "!edit"] })
  expect([except.toolMode, except.tools]).toEqual(["except", ["bash", "edit"]])
  expect(configOf(except, { tools: ["!bash", "!edit"] })).toMatchObject({ tools: ["!bash", "!edit"] })
  expect(
    configOf({ ...only, toolMode: "all", tools: [] }, { tools: ["read", "grep"] }),
    "every tool over a narrowed agent is stored as *",
  ).toMatchObject({ tools: ["*"] })
  expect(configOf({ ...only, tools: [] }, {}), "only these, with none chosen").toBeString()
  expect(configOf(draftOf({}), {})).not.toHaveProperty("tools")
})

test("the form refuses what the engine would not apply and drops rules left blank", async () => {
  const { configOf, draftOf } = await import("../src/ui/settings-prompts")
  expect(configOf({ ...draftOf({}), steps: "0" }, {}), "steps must be positive").toBeString()
  expect(configOf({ ...draftOf({}), steps: "50", variant: "high" }, {})).toEqual({
    prompt: "",
    steps: 50,
    variant: "high",
  })
  const rules = [
    { kind: "bash", pattern: "git push*", decision: "deny" as const },
    { kind: "bash", pattern: " ", decision: "ask" as const },
  ]
  expect(configOf({ ...draftOf({}), permissions: rules }, {})).toMatchObject({ permissions: [rules[0]] })
})

test("reasoning levels come from the pinned model, else from every connected model, in order", async () => {
  const { reasoningLevels } = await import("../src/ui/settings-prompts")
  const withLevels = (id: string, levels: string[]) => ({
    ...model(id),
    variants: Object.fromEntries(levels.map((level) => [level, {}])),
  })
  const state = {
    providers: [provider("openai", [withLevels("fast", ["low", "high"]), withLevels("deep", ["xhigh", "medium"])])],
    connected: ["openai"],
  }
  expect(reasoningLevels(state, "", "")).toEqual(["low", "medium", "high", "xhigh"])
  expect(reasoningLevels(state, "openai/fast", "")).toEqual(["low", "high"])
  expect(reasoningLevels(state, "openai/fast", "max"), "a saved level stays offered").toEqual(["low", "high", "max"])
})

test("agents are listed as picked in the composer, delegated to, then run by Drift itself", async () => {
  const { agentGroups } = await import("../src/ui/settings-prompts")
  const agent = (name: string, mode: "primary" | "subagent" | "all", hidden = false) => ({
    name,
    description: "",
    mode,
    hidden,
    builtIn: true,
    tools: [],
  })
  const groups = agentGroups([
    agent("plan", "primary"),
    agent("title", "primary", true),
    agent("explore", "subagent"),
    agent("build", "all"),
    agent("general", "subagent"),
  ])
  expect(groups.map((group) => group.agents.map((item) => item.name))).toEqual([
    ["build", "plan"],
    ["explore", "general"],
    ["title"],
  ])
})

test("the behavior editor refuses what the engine would not apply, naming the field", () => {
  expect(agentBehaviorIssue({ model: "provider/model", steps: 8, tools: ["read"] })).toBeUndefined()
  expect(agentBehaviorIssue({ model: "" })).toBeUndefined()
  for (const [behavior, field] of [
    [{ temperature: 0.2 }, "temperature"],
    [{ permission: { edit: "deny" } }, "permission"],
    [{ variant: 3 }, "variant"],
    [{ permissions: [{ kind: "read", pattern: "*", decision: "invalid" }] }, "permissions"],
    [{ steps: 0 }, "steps"],
    [{ steps: 1.5 }, "steps"],
    [{ tools: [] }, "tools"],
    [{ tools: { bash: false } }, "tools"],
  ] as const) {
    expect(agentBehaviorIssue(behavior as Record<string, unknown>)).toBe(field)
  }
})

test("a stored override keeps only fields the engine still applies", () => {
  expect(
    applicableOverride({ prompt: "p", model: "a/b", steps: 3, tools: ["read"], color: "#fff", permission: "deny" }),
  ).toEqual({
    prompt: "p",
    model: "a/b",
    steps: 3,
    tools: ["read"],
  })
})
