/**
 * Finding text inside the open conversation.
 *
 * The transcript is virtualized, so most rows have no DOM at any moment and the browser's own find
 * would only ever see the handful that are mounted. Matching therefore runs over the message data
 * and reports message ids, which the timeline can scroll to and highlight.
 */

import type { MessageEntry } from "../engine/store"

export type TranscriptMatch = { messageId: string; count: number }

/** A single occurrence: which message it is in and which occurrence within that message. */
export type TranscriptOccurrence = { messageId: string; index: number }

type TextualPart = { type: string; text?: string; synthetic?: boolean; state?: { output?: string } }

/**
 * All readable text in a message: what the user wrote, what the assistant replied, its reasoning,
 * and any tool output still retained. Synthetic parts are Drift's own scaffolding and are skipped,
 * because a match there is not something the user ever saw.
 */
export function entrySearchText(entry: MessageEntry) {
  const parts: string[] = []
  for (const part of entry.parts as unknown as TextualPart[]) {
    if (part.synthetic) continue
    if (part.type === "text" || part.type === "reasoning") {
      if (part.text) parts.push(part.text)
      continue
    }
    if (part.type === "tool" && typeof part.state?.output === "string") parts.push(part.state.output)
  }
  return parts.join("\n")
}

/**
 * Lowercased search text per entry, kept between keystrokes.
 *
 * Joining and lowercasing every message is the expensive part of a search: on a long session it
 * allocates the entire transcript again per run. Message text only ever changes by growing (a
 * streaming reply, tool output landing), so a total-length fingerprint is enough to know when the
 * cached copy is stale without comparing the text itself.
 */
const loweredCache = new WeakMap<MessageEntry, { fingerprint: number; lower: string }>()

function textFingerprint(entry: MessageEntry) {
  let total = entry.parts.length
  for (const part of entry.parts as unknown as TextualPart[]) {
    if (part.synthetic) continue
    if (part.type === "text" || part.type === "reasoning") total += part.text?.length ?? 0
    else if (part.type === "tool" && typeof part.state?.output === "string") total += part.state.output.length
  }
  return total
}

export function loweredSearchText(entry: MessageEntry) {
  const fingerprint = textFingerprint(entry)
  const cached = loweredCache.get(entry)
  if (cached && cached.fingerprint === fingerprint) return cached.lower
  const lower = entrySearchText(entry).toLowerCase()
  loweredCache.set(entry, { fingerprint, lower })
  return lower
}

function countIn(lowerHaystack: string, lowerNeedle: string) {
  let count = 0
  for (let at = lowerHaystack.indexOf(lowerNeedle); at !== -1; at = lowerHaystack.indexOf(lowerNeedle, at + lowerNeedle.length))
    count++
  return count
}

export function countOccurrences(text: string, query: string) {
  const needle = query.toLowerCase()
  if (!needle) return 0
  return countIn(text.toLowerCase(), needle)
}

/** Messages containing `query`, in transcript order, with how many times each one matches. */
export function transcriptMatches(entries: MessageEntry[], query: string): TranscriptMatch[] {
  const needle = query.trim().toLowerCase()
  if (!needle) return []
  const found: TranscriptMatch[] = []
  for (const entry of entries) {
    const count = countIn(loweredSearchText(entry), needle)
    if (count > 0) found.push({ messageId: entry.info.id, count })
  }
  return found
}

export function totalMatches(matches: TranscriptMatch[]) {
  return matches.reduce((total, match) => total + match.count, 0)
}

/**
 * Resolves a flat occurrence index to the message it falls in.
 *
 * Navigation steps through occurrences rather than messages so the position label ("3 of 17") and
 * the Enter key agree: pressing Enter never skips over repeats inside one long message.
 */
export function occurrenceAt(matches: TranscriptMatch[], cursor: number): TranscriptOccurrence | undefined {
  if (cursor < 0) return undefined
  let before = 0
  for (const match of matches) {
    if (cursor < before + match.count) return { messageId: match.messageId, index: cursor - before }
    before += match.count
  }
  return undefined
}

/**
 * Moves the cursor by `step`, wrapping at both ends.
 *
 * Wrapping is what makes repeated Enter usable: reaching the last match should return to the first
 * rather than stop. An empty result set has no cursor at all.
 */
export function stepMatch(current: number, total: number, step: number) {
  if (total <= 0) return -1
  return (((current + step) % total) + total) % total
}

/**
 * Keeps the cursor pointing at the same place when the result set changes underneath it.
 *
 * Results change while a reply streams in and when older pages load. Re-anchoring by message id
 * stops the current position jumping to an unrelated match every time the transcript grows.
 */
export function reanchorMatch(
  matches: TranscriptMatch[],
  previous: TranscriptOccurrence | undefined,
  fallback: number,
) {
  const total = totalMatches(matches)
  if (!total) return -1
  if (previous) {
    let before = 0
    for (const match of matches) {
      if (match.messageId === previous.messageId) return before + Math.min(previous.index, match.count - 1)
      before += match.count
    }
  }
  return Math.min(Math.max(fallback, 0), total - 1)
}
