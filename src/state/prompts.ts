import { shellInvoke } from "../shell"

export type PromptFamily = { id: string; original: string; default: string }
export type PromptCatalogAgent = { name: string; prompt: string }
export type PromptCatalog = { version: number; families: PromptFamily[]; agents: PromptCatalogAgent[] }
export type PromptOverride = { key: string; value: unknown; original?: unknown; updatedAt: number }
export type PromptSnapshot = { catalog: PromptCatalog; overrides: PromptOverride[] }

export function loadPromptSnapshot() {
  const invoke = shellInvoke()
  return invoke ? invoke<PromptSnapshot>("prompt_snapshot") : Promise.resolve<PromptSnapshot | null>(null)
}

export async function savePromptOverride(key: string, value: unknown, original?: unknown) {
  const invoke = shellInvoke()
  if (!invoke) throw new Error("Prompt editing requires the Drift desktop backend")
  await invoke("prompt_save", { key, value, original })
}

export async function resetPromptOverride(key: string) {
  const invoke = shellInvoke()
  if (!invoke) throw new Error("Prompt editing requires the Drift desktop backend")
  await invoke("prompt_reset", { key })
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
