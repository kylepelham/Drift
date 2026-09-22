import type { EngineState } from "../engine/store"
import { lmStudioModelReady } from "./lm-studio"
import { orderedModelProviderIds } from "./prefs"

export function subagentModelOptions(state: Pick<EngineState, "providers" | "connected">) {
  const providers = state.providers.filter((provider) => state.connected.includes(provider.id))
  return orderedModelProviderIds(providers.map((provider) => provider.id)).flatMap((id) => {
    const provider = providers.find((item) => item.id === id)!
    return Object.values(provider.models)
      .filter((model) => provider.id === "lmstudio" ? lmStudioModelReady(model) : model.capabilities.toolcall)
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
