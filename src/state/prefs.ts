import type { ModelRef } from "../engine/store"
import { persisted } from "./persist"

export const [modelPref, setModelPref] = persisted<ModelRef | null>("drift.model", null)
export const [agentPref, setAgentPref] = persisted<string>("drift.agent", "build")
export const [variantPref, setVariantPref] = persisted<string | null>("drift.variant", null)
export const [hiddenModelIds, setHiddenModelIds] = persisted<string[]>("drift.models.hidden", [])
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
  setHiddenModelIds(visible ? hiddenModelIds().filter((item) => item !== id) : [...new Set([...hiddenModelIds(), id])])
}
