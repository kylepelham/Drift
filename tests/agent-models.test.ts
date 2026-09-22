import { expect, test } from "bun:test"
import type { ModelInfo, ProviderInfo } from "../src/engine/store"
import { agentBehaviorModel, subagentModelOptions, withAgentModel } from "../src/state/agent-models"
import { agentOverrideValue } from "../src/state/prompts"
import { hiddenModelIds, setHiddenModelIds } from "../src/state/prefs"

function model(id: string, name = id, toolcall = true, context = 65536): ModelInfo {
  return { id, name, capabilities: { toolcall }, limit: { context } } as ModelInfo
}

function provider(id: string, models: ModelInfo[]): ProviderInfo {
  return { id, name: id.toUpperCase(), models: Object.fromEntries(models.map((item) => [item.id, item])) }
}

test("subagent model choices include hidden tool models and preserve provider-qualified IDs", () => {
  const hidden = hiddenModelIds()
  setHiddenModelIds(["one/cheap"])
  try {
    const providers = [
      provider("one", [model("vendor/review", "Smart"), model("cheap", "Cheap"), model("embed", "Embedding", false)]),
      provider("two", [model("cheap", "Cheap")]),
      provider("offline", [model("unavailable")]),
    ]
    expect(subagentModelOptions({ providers, connected: ["one", "two"] })).toEqual([
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
  expect(subagentModelOptions({ providers, connected: ["lmstudio"] }).map((item) => item.id)).toEqual(["lmstudio/ready"])
  expect(subagentModelOptions({ providers, connected: [] })).toEqual([])
})

test("model selection preserves prompt, permissions, and custom behavior", () => {
  const config = { prompt: "Review carefully", permission: { edit: "deny" }, temperature: 0.2, vendorOption: { enabled: true } }
  const selected = withAgentModel(JSON.stringify(config), "provider/vendor/reviewer")
  expect(JSON.parse(selected)).toEqual({ ...config, model: "provider/vendor/reviewer" })
  expect(agentBehaviorModel(selected)).toBe("provider/vendor/reviewer")
  expect(agentOverrideValue(JSON.parse(selected), config)).toEqual({ model: "provider/vendor/reviewer" })
})

test("Current model is dynamic inheritance and explicitly masks an underlying model pin", () => {
  expect(agentBehaviorModel("{}")).toBe("")
  const baseline = { model: "provider/smart", prompt: "Keep prompt", mode: "subagent" }
  const inherited = withAgentModel(JSON.stringify(baseline), "")
  expect(agentBehaviorModel(inherited)).toBe("")
  expect(JSON.parse(inherited)).toEqual({ ...baseline, model: "" })
  const override = agentOverrideValue(JSON.parse(inherited), baseline, { prompt: "Keep prompt", model: "provider/smart" })
  expect(override).toEqual({ prompt: "Keep prompt", model: "" })
  expect(agentBehaviorModel(JSON.stringify({ ...baseline, ...override }))).toBe("")
})

test("editing behavior JSON updates the model selection, including unavailable saved models", () => {
  expect(agentBehaviorModel('{"model":"removed-provider/old-model"}')).toBe("removed-provider/old-model")
  expect(agentBehaviorModel('{"model":"different-provider/new-model"}')).toBe("different-provider/new-model")
  expect(agentBehaviorModel('{"prompt":"No pin"}')).toBe("")
})

test.each(["{", "null", "[]", '"string"', "42"])("choosing a model does not replace invalid behavior JSON: %s", (behavior) => {
  expect(agentBehaviorModel(behavior)).toBeUndefined()
  expect(() => withAgentModel(behavior, "provider/model")).toThrow()
})
