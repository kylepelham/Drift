/**
 * Finding text inside the open conversation.
 *
 * The transcript is virtualized, so most rows have no DOM at any moment and the browser's own find
 * would only ever see the handful that are mounted. Matching therefore runs over the message data
 * and reports message ids, which the timeline can scroll to and highlight.
 */

import type { MessageEntry } from "../engine/store"

export type TranscriptMatch = { messageId: string; count: number }

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

export function countOccurrences(text: string, query: string) {
  const needle = query.toLowerCase()
  if (!needle) return 0
  const haystack = text.toLowerCase()
  let count = 0
  for (let at = haystack.indexOf(needle); at !== -1; at = haystack.indexOf(needle, at + needle.length)) count++
  return count
}

/** Messages containing `query`, in transcript order, with how many times each one matches. */
export function transcriptMatches(entries: MessageEntry[], query: string): TranscriptMatch[] {
  const needle = query.trim()
  if (!needle) return []
  const found: TranscriptMatch[] = []
  for (const entry of entries) {
    const count = countOccurrences(entrySearchText(entry), needle)
    if (count > 0) found.push({ messageId: entry.info.id, count })
  }
  return found
}

export function totalMatches(matches: TranscriptMatch[]) {
  return matches.reduce((total, match) => total + match.count, 0)
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
 * Keeps the cursor pointing at the same message when the result set changes underneath it.
 *
 * Results change while a reply streams in and when older pages load. Re-anchoring by message id
 * stops the current position jumping to an unrelated match every time the transcript grows.
 */
export function reanchorMatch(matches: TranscriptMatch[], previousId: string | undefined, fallback: number) {
  if (!matches.length) return -1
  const previous = previousId ? matches.findIndex((match) => match.messageId === previousId) : -1
  if (previous >= 0) return previous
  return Math.min(Math.max(fallback, 0), matches.length - 1)
}
