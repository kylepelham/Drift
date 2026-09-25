import type { EngineState } from "../engine/store"
import { lmStudioMinimumContext, lmStudioModelReady } from "./lm-studio"
import { orderedModelProviderIds } from "./prefs"

export function agentModelCapability(
  agent: { name: string; mode: string } | undefined,
): "tools" | "text" | undefined {
  if (agent?.name === "title" || agent?.name === "compaction") return "text"
  if (agent?.mode === "subagent" || agent?.mode === "all") return "tools"
}

export function agentModelOptions(
  state: Pick<EngineState, "providers" | "connected">,
  capability: "tools" | "text" = "tools",
) {
  const providers = state.providers.filter((provider) => state.connected.includes(provider.id))
  return orderedModelProviderIds(providers.map((provider) => provider.id)).flatMap((id) => {
    const provider = providers.find((item) => item.id === id)!
    return Object.values(provider.models)
      .filter((model) => {
        if (provider.id === "lmstudio") {
          if (capability === "tools" && !lmStudioModelReady(model)) return false
          if (capability === "text" && model.limit.context < lmStudioMinimumContext) return false
        }
        if (capability === "tools") return model.capabilities.toolcall
        return model.capabilities.input.text && model.capabilities.output.text
      })
      .sort((a, b) => a.name.localeCompare(b.name))
      .map((model) => ({
        id: `${provider.id}/${model.id}`,
        label: model.name,
        group: provider.name,
        detail: model.id,
      }))
  })
}

export function agentBehaviorModel(behavior: string): string | undefined {
  try {
    const config = parseAgentBehavior(behavior)
    return typeof config.model === "string" ? config.model : ""
  } catch {
    return undefined
  }
}

export function withAgentModel(behavior: string, model: string) {
  const config = parseAgentBehavior(behavior)
  // An empty model masks lower-precedence agent pins and restores parent-model inheritance.
  return JSON.stringify({ ...config, model }, null, 2)
}

function parseAgentBehavior(behavior: string): Record<string, unknown> {
  const config: unknown = JSON.parse(behavior)
  if (!config || typeof config !== "object" || Array.isArray(config)) throw new Error("Invalid agent configuration")
  return config as Record<string, unknown>
}
