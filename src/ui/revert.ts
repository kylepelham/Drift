import { messageText, nextUserMessage, type MessageEntry } from "../engine/store"
import { clearComposerDraft, composerScope, draftFromMessage, setComposerDraft } from "../state/composer"

export type RevertHost = {
  state: { transcripts: Record<string, MessageEntry[]> }
  actions: {
    revert: (id: string, messageID: string) => Promise<boolean>
    unrevert: (id: string) => Promise<boolean>
  }
}

export function revertDockEntries(entries: MessageEntry[], marker?: string) {
  if (!marker) return []
  return entries
    .filter((entry) => entry.info.role === "user" && entry.info.id >= marker)
    .sort((a, b) => b.info.id.localeCompare(a.info.id))
}

export function revertPreview(entry?: MessageEntry) {
  if (!entry) return ""
  return messageText(entry).replace(/\s+/g, " ").trim()
}

// Restores the given undone user message: the revert marker moves to the next user
// message, or the session unreverts entirely when the message is the newest one.
export async function restoreReverted(engine: RevertHost, sessionID: string, messageID: string) {
  const next = nextUserMessage(engine.state.transcripts[sessionID] ?? [], messageID)
  if (next) {
    const restored = draftFromMessage(next)
    const success = await engine.actions.revert(sessionID, next.info.id)
    if (success) setComposerDraft(composerScope(sessionID), restored)
    return success
  }
  const success = await engine.actions.unrevert(sessionID)
  if (success) clearComposerDraft(composerScope(sessionID))
  return success
}
