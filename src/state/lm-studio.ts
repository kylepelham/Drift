import type { ModelInfo } from "../engine/store"

export const lmStudioMinimumContext = 32_768

export function formatModelContext(tokens: number) {
  if (tokens < 1_024) return `${tokens}`
  const value = tokens / 1_024
  return `${Number.isInteger(value) ? value : value.toFixed(1)}K`
}

export function lmStudioModelReady(model: ModelInfo) {
  return model.capabilities.toolcall && model.limit.context >= lmStudioMinimumContext
}
