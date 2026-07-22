import { createSignal } from "solid-js"
import type { MessageEntry } from "../engine/store"

export type StagedFile = { id: string; filename: string; mime: string; dataUrl: string; size: number }
export type ComposerDraft = { text: string; staged: StagedFile[]; mentions: string[] }

const emptyDraft: ComposerDraft = { text: "", staged: [], mentions: [] }
const [drafts, setDrafts] = createSignal<Record<string, ComposerDraft>>({})

export function composerScope(sessionId?: string | null, workspaceId?: string | null) {
  return sessionId ? `session:${sessionId}` : `new:${workspaceId ?? "none"}`
}

export function composerDraft(scope: string) {
  return drafts()[scope] ?? emptyDraft
}

export function patchComposerDraft(scope: string, patch: Partial<ComposerDraft>) {
  setDrafts({ ...drafts(), [scope]: { ...composerDraft(scope), ...patch } })
}

export function setComposerDraft(scope: string, draft: ComposerDraft) {
  setDrafts({ ...drafts(), [scope]: draft })
}

export function clearComposerDraft(scope: string) {
  const next = { ...drafts() }
  delete next[scope]
  setDrafts(next)
}

export function draftFromMessage(entry: MessageEntry): ComposerDraft {
  const text = entry.parts
    .flatMap((part) => (part.type === "text" && !part.synthetic && !part.ignored ? [part.text] : []))
    .join("\n")
  const staged: StagedFile[] = []
  const mentions: string[] = []
  for (const part of entry.parts) {
    if (part.type !== "file") continue
    if (part.source?.type === "file") {
      const value = part.source.text.value
      if (value.startsWith("@")) mentions.push(value.slice(1))
      continue
    }
    if (!part.url.startsWith("data:")) continue
    staged.push({
      id: part.id,
      filename: part.filename ?? "attachment",
      mime: part.mime,
      dataUrl: part.url,
      size: 0,
    })
  }
  return { text, staged, mentions: [...new Set(mentions)] }
}
