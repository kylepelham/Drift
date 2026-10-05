import { childrenOf, sessionBusy, type EngineState } from "../engine/store"

/**
 * Which sessions wait on the user. The engine answers what auto-accept and "always" cover before
 * an ask is ever published, so every ask that arrives waits on the user until it is answered.
 */

/** A session is waiting on the user when it has an unanswered question or permission. */
export function sessionNeedsAttention(state: EngineState, id: string) {
  return (state.questions[id]?.length ?? 0) > 0 || (state.permissions[id]?.length ?? 0) > 0
}

/** Subagents appear under their parent while they run, wait on the user, or are open; finished work lives in the task card. */
export function sidebarWorkers(state: EngineState, parentId: string, selected?: string | null) {
  return childrenOf(state, parentId).filter(
    (child) => child.id === selected || sessionBusy(state, child.id) || sessionNeedsAttention(state, child.id),
  )
}
