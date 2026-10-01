import { backendInvoke } from "../backend"

export type PromptFamily = { id: string; original: string; default: string; variants?: PromptFamily[] }
export type PromptCatalogAgent = { name: string; prompt: string }
export type PromptCatalog = { version: number; families: PromptFamily[]; agents: PromptCatalogAgent[] }
export type PromptOverride = { key: string; value: unknown; original?: unknown; updatedAt: number }
export type PromptSnapshot = { catalog: PromptCatalog; overrides: PromptOverride[] }

export function loadPromptSnapshot() {
  const invoke = backendInvoke()
  return invoke ? invoke<PromptSnapshot>("prompt_snapshot") : Promise.resolve<PromptSnapshot | null>(null)
}

export async function savePromptOverride(key: string, value: unknown, original?: unknown) {
  const invoke = backendInvoke()
  if (!invoke) throw new Error("Prompt editing requires the Drift host backend")
  await invoke("prompt_save", { key, value, original })
}

export async function resetPromptOverride(key: string) {
  const invoke = backendInvoke()
  if (!invoke) throw new Error("Prompt editing requires the Drift host backend")
  await invoke("prompt_reset", { key })
}

/** What the engine applies from the behavior editor; the prompt has its own editor. Anything else is refused. */
export const agentBehaviorFields = ["model", "steps", "tools"] as const

/** The first field the engine would not apply as written, or nothing when every one is valid. */
export function agentBehaviorIssue(behavior: Record<string, unknown>): string | undefined {
  const unknown = Object.keys(behavior).find((key) => !(agentBehaviorFields as readonly string[]).includes(key))
  if (unknown) return unknown
  if ("model" in behavior && typeof behavior.model !== "string") return "model"
  const steps = behavior.steps
  if (steps !== undefined && !(typeof steps === "number" && Number.isInteger(steps) && steps > 0)) return "steps"
  const tools = behavior.tools
  if (tools !== undefined && !(Array.isArray(tools) && tools.length > 0 && tools.every((tool) => typeof tool === "string"))) return "tools"
}

/** A stored override keeps only what the engine still applies, so saving never re-sends retired fields. */
export function applicableOverride(value: Record<string, unknown>) {
  const kept = new Set<string>(["prompt", ...agentBehaviorFields])
  return Object.fromEntries(Object.entries(value).filter(([key]) => kept.has(key)))
}

export function agentOverrideValue(
  config: Record<string, unknown>,
  baseline: Record<string, unknown>,
  existing: Record<string, unknown> = {},
) {
  const result = { ...existing }
  for (const key of new Set([...Object.keys(config), ...Object.keys(baseline)])) {
    if (jsonEqual(config[key], baseline[key])) continue
    if (key in config) result[key] = config[key]
    else delete result[key]
  }
  return result
}

function jsonEqual(left: unknown, right: unknown): boolean {
  if (left === right) return true
  if (!left || !right || typeof left !== "object" || typeof right !== "object") return false
  if (Array.isArray(left) || Array.isArray(right)) {
    return Array.isArray(left) && Array.isArray(right) && left.length === right.length && left.every((item, index) => jsonEqual(item, right[index]))
  }
  const leftEntries = Object.entries(left)
  const rightRecord = right as Record<string, unknown>
  return leftEntries.length === Object.keys(rightRecord).length && leftEntries.every(([key, value]) => key in rightRecord && jsonEqual(value, rightRecord[key]))
}
