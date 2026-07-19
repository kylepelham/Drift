import type { ModelRef } from "../engine/store"
import { persisted } from "./persist"

export const [modelPref, setModelPref] = persisted<ModelRef | null>("drift.model", null)
export const [agentPref, setAgentPref] = persisted<string>("drift.agent", "build")
export const [variantPref, setVariantPref] = persisted<string | null>("drift.variant", null)
export const [hiddenModelIds, setHiddenModelIds] = persisted<string[]>("drift.models.hidden", [])

export function setModelVisible(id: string, visible: boolean) {
  setHiddenModelIds(visible ? hiddenModelIds().filter((item) => item !== id) : [...new Set([...hiddenModelIds(), id])])
}
