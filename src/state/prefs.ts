import type { ModelRef } from "../engine/store"
import { persisted } from "./persist"

export const [modelPref, setModelPref] = persisted<ModelRef | null>("drift.model", null)
export const [agentPref, setAgentPref] = persisted<string>("drift.agent", "build")
export const [variantPref, setVariantPref] = persisted<string | null>("drift.variant", null)
export const [hiddenModelIds, setHiddenModelIds] = persisted<string[]>("drift.models.hidden", [])
export const [shownModelIds, setShownModelIds] = persisted<string[]>("drift.models.shown", [])
export const [modelProviderOrder, setModelProviderOrder] = persisted<string[]>("drift.models.providerOrder", [])
export const [showReasoning, setShowReasoning] = persisted<boolean>("drift.reasoning", false)
export const [notifyAttention, setNotifyAttention] = persisted<boolean>("drift.notifications", false)
export const [collapseCompaction, setCollapseCompaction] = persisted<boolean>("drift.compaction.collapsible", true)
export const [compactionCollapsed, setCompactionCollapsed] = persisted<boolean>("drift.compaction.collapsed", true)
export const [autoUpdate, setAutoUpdate] = persisted<boolean>("drift.autoUpdate", true)

export const [autoAcceptSessions, setAutoAcceptSessions] = persisted<string[]>("drift.autoAccept", [])

export function toggleAutoAccept(sessionId: string) {
  const current = autoAcceptSessions()
  setAutoAcceptSessions(
    current.includes(sessionId) ? current.filter((id) => id !== sessionId) : [...current, sessionId],
  )
}

type SessionPrefs = { model?: ModelRef | null; agent?: string; variant?: string | null }
const [sessionPrefs, setSessionPrefs] = persisted<Record<string, SessionPrefs>>("drift.session.prefs", {})

export function prefsFor(sessionId: string | null | undefined) {
  const own = (sessionId && sessionPrefs()[sessionId]) || {}
  return {
    model: own.model !== undefined ? own.model : modelPref(),
    agent: own.agent ?? agentPref(),
    variant: own.variant !== undefined ? own.variant : variantPref(),
  }
}

export function updatePrefs(sessionId: string | null | undefined, patch: SessionPrefs) {
  if (patch.model !== undefined) setModelPref(patch.model)
  if (patch.agent !== undefined) setAgentPref(patch.agent)
  if (patch.variant !== undefined) setVariantPref(patch.variant)
  if (sessionId) setSessionPrefs({ ...sessionPrefs(), [sessionId]: { ...sessionPrefs()[sessionId], ...patch } })
}

export function seedPrefs(sessionId: string) {
  if (sessionPrefs()[sessionId]) return
  setSessionPrefs({
    ...sessionPrefs(),
    [sessionId]: { model: modelPref(), agent: agentPref(), variant: variantPref() },
  })
}

export function setModelVisible(id: string, visible: boolean) {
  setModelsVisible([id], visible)
}

export function setModelsVisible(ids: string[], visible: boolean) {
  const changed = new Set(ids)
  setHiddenModelIds(visible ? hiddenModelIds().filter((id) => !changed.has(id)) : [...new Set([...hiddenModelIds(), ...ids])])
  setShownModelIds(visible ? [...new Set([...shownModelIds(), ...ids])] : shownModelIds().filter((id) => !changed.has(id)))
}

export function modelVisible(id: string, defaultVisible: boolean) {
  if (shownModelIds().includes(id)) return true
  if (hiddenModelIds().includes(id)) return false
  return defaultVisible
}

export function mergeModelProviderOrder(saved: string[], available: string[]) {
  const unique = [...new Set(available)]
  return [...saved.filter((id) => unique.includes(id)), ...unique.filter((id) => !saved.includes(id))]
}

export function orderedModelProviderIds(available: string[]) {
  return mergeModelProviderOrder(modelProviderOrder(), available)
}

export function reorderModelProviderIds(order: string[], id: string, beforeID: string | null) {
  if (!order.includes(id) || id === beforeID) return order
  const next = order.filter((item) => item !== id)
  const target = beforeID ? next.indexOf(beforeID) : next.length
  next.splice(target < 0 ? next.length : target, 0, id)
  return next
}

export function moveModelProvider(id: string, beforeID: string | null, available: string[]) {
  setModelProviderOrder(reorderModelProviderIds(orderedModelProviderIds(available), id, beforeID))
}
