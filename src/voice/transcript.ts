/** Punctuation that belongs to the preceding word, so no space is inserted before it. */
const trailing = new Set([",", ".", "!", "?", ";", ":", ")", "]", "}", "%", "'", '"'])
/** Characters that open a group, so the next word follows immediately. */
const opening = new Set(["(", "[", "{", '"', "'", "@", "#", "/", "-", "_"])

/** Joins a finalized speech segment onto the draft without disturbing what is already there. */
export function appendDictation(existing: string, segment: string) {
  const text = segment.trim()
  if (!text) return existing
  if (!existing) return text
  if (/\s$/.test(existing)) return existing + text
  if (trailing.has(text[0]!)) return existing + text
  if (opening.has(existing.at(-1)!)) return existing + text
  return `${existing} ${text}`
}

/** Markers whisper emits for audio it decided was not speech. */
const nonSpeech = /\[(blank_audio|inaudible|silence|music|applause|laughter|noise)\]/gi

/**
 * Collapses the sidecar's line-per-segment output into one phrase. A result that is nothing but a
 * bracketed marker is dropped, which is how a pause stops becoming invented text.
 */
export function cleanTranscript(raw: string) {
  const collapsed = raw
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .join(" ")
  const stripped = collapsed.replace(nonSpeech, " ").replace(/\s+/g, " ").trim()
  return /^[[(][^\])]*[\])]$/.test(stripped) ? "" : stripped
}

export function formatDictationElapsed(ms: number) {
  const total = Math.max(0, Math.floor(ms / 1000))
  const minutes = Math.floor(total / 60)
  return `${minutes}:${String(total % 60).padStart(2, "0")}`
}
