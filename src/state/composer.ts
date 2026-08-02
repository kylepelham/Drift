import { createSignal } from "solid-js"
import type { MessageEntry } from "../engine/store"
import { resolveAttachmentKind, type StagedAttachment } from "../attachments"
import { persisted } from "./persist"

export type StagedFile = StagedAttachment
export type ComposerDraft = { text: string; staged: StagedFile[]; mentions: string[] }
export type ComposerHistoryEntry = Pick<ComposerDraft, "text" | "mentions">
export type ComposerHistoryNavigation = { index: number; saved: ComposerDraft | null }

const emptyDraft: ComposerDraft = { text: "", staged: [], mentions: [] }
export const maxComposerHistory = 100
const [drafts, setDrafts] = createSignal<Record<string, ComposerDraft>>({})
const [history, setHistory] = persisted<ComposerHistoryEntry[]>("drift.composer.history", [], normalizeComposerHistory)

export const composerHistory = history

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

export function clearComposerDraft(scope: string, expected?: ComposerDraft) {
  if (expected && composerDraft(scope) !== expected) return false
  const next = { ...drafts() }
  delete next[scope]
  setDrafts(next)
  return true
}

export function migrateComposerDraft(from: string, to: string) {
  if (from === to) return
  const next = { ...drafts() }
  const draft = next[from]
  delete next[from]
  if (draft) next[to] = draft
  setDrafts(next)
}

export function recordComposerHistory(draft: ComposerDraft) {
  setHistory(prependComposerHistory(history(), draft))
}

export function prependComposerHistory(entries: ComposerHistoryEntry[], draft: ComposerDraft) {
  const text = draft.text.trim()
  if (!text) return entries
  const entry = { text, mentions: [...new Set(draft.mentions)] }
  const first = entries[0]
  if (first?.text === entry.text && sameStrings(first.mentions, entry.mentions)) return entries
  return [entry, ...entries].slice(0, maxComposerHistory)
}

export function normalizeComposerHistory(value: unknown) {
  if (!Array.isArray(value)) return []
  return value
    .flatMap((entry) => {
      if (!entry || typeof entry !== "object") return []
      const record = entry as Record<string, unknown>
      if (typeof record.text !== "string") return []
      const text = record.text.trim()
      if (!text) return []
      const mentions: string[] = Array.isArray(record.mentions)
        ? [...new Set(record.mentions.filter((mention: unknown): mention is string => typeof mention === "string"))]
        : []
      return [{ text, mentions } satisfies ComposerHistoryEntry]
    })
    .slice(0, maxComposerHistory)
}

export function canNavigateComposerHistory(direction: "up" | "down", text: string, cursor: number, inHistory: boolean) {
  const position = Math.max(0, Math.min(cursor, text.length))
  if (inHistory) return position === 0 || position === text.length
  if (direction === "up") return position === 0 && text.length === 0
  return position === text.length
}

export function navigateComposerHistory(
  entries: ComposerHistoryEntry[],
  navigation: ComposerHistoryNavigation,
  current: ComposerDraft,
  direction: "up" | "down",
) {
  if (direction === "up") {
    const index = navigation.index + 1
    const entry = entries[index]
    if (!entry) return
    return {
      navigation: { index, saved: navigation.index === -1 ? cloneDraft(current) : navigation.saved },
      draft: historyDraft(entry),
      cursor: "start" as const,
    }
  }
  if (navigation.index < 0) return
  const index = navigation.index - 1
  if (index >= 0) {
    return {
      navigation: { index, saved: navigation.saved },
      draft: historyDraft(entries[index]),
      cursor: "end" as const,
    }
  }
  return {
    navigation: { index: -1, saved: null },
    draft: cloneDraft(navigation.saved ?? emptyDraft),
    cursor: "end" as const,
  }
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
    const resolved = resolveAttachmentKind({ filename: part.filename, mime: part.mime })
    if (resolved.kind === "unsupported") continue
    staged.push({
      id: part.id,
      filename: part.filename ?? "attachment",
      mime: resolved.mime,
      dataUrl: part.url,
      size: 0,
      status: "ready",
      meta: {},
    })
  }
  return { text, staged, mentions: [...new Set(mentions)] }
}

function historyDraft(entry: ComposerHistoryEntry): ComposerDraft {
  return { text: entry.text, staged: [], mentions: [...entry.mentions] }
}

function cloneDraft(draft: ComposerDraft): ComposerDraft {
  return { text: draft.text, staged: [...draft.staged], mentions: [...draft.mentions] }
}

function sameStrings(left: string[], right: string[]) {
  return left.length === right.length && left.every((value, index) => value === right[index])
}
