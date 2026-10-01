import type { Queued } from "../engine/native/client"
import { agentLabel, reasoningLevelLabel, t } from "../state/i18n"

/** What the composer says about a prompt waiting for the turn: who it runs as (and on which model, when that changes), or why it could not start. */
export function queuedNotice(queued: Queued, newModel?: string) {
  const choice = [agentLabel(queued.agent), queued.variant && reasoningLevelLabel(queued.variant), newModel].filter(Boolean).join(", ")
  if (queued.error) return { failed: true, text: t("drift.queued.failed", { choice, error: queued.error }) }
  return { failed: false, text: t("drift.queued.waiting", { choice }) }
}
