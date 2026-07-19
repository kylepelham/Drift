import type { ModelRef } from "../engine/store"
import { persisted } from "./persist"

export const [modelPref, setModelPref] = persisted<ModelRef | null>("drift.model", null)
export const [agentPref, setAgentPref] = persisted<string>("drift.agent", "build")
export const [variantPref, setVariantPref] = persisted<string | null>("drift.variant", null)
