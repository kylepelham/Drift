export const responseAnimationInterruptEvent = "drift:response-animation-interrupt"
/** A restrained typewriter pace that normally reveals about one character per display frame. */
export const responseRevealCharsPerSecond = 144
/** Backlog past which typing gradually accelerates, capped at twice the normal pace. */
export const responseRevealMaxBacklogChars = 600

/**
 * Keeps a character reveal from splitting a UTF-16 surrogate pair.
 */
export function revealBoundary(text: string, from: number, limit: number) {
  if (limit >= text.length) return text.length
  if (limit <= from) return from
  const previous = text.charCodeAt(limit - 1)
  const current = text.charCodeAt(limit)
  const splitsSurrogate = previous >= 0xd800 && previous <= 0xdbff && current >= 0xdc00 && current <= 0xdfff
  return splitsSurrogate ? limit + 1 : limit
}

export type RevealStep = {
  revealed: number
  target: string
  elapsedMs: number
  charsPerSecond?: number
  maxBacklogChars?: number
}

/**
 * Advances the revealed length by one frame's worth of characters.
 *
 * The budget comes from elapsed wall-clock time rather than from how much text arrived, so a
 * provider that streams one large block and a provider that streams single characters reveal at
 * the same visual pace.
 */
export function revealStep(step: RevealStep) {
  const backlog = step.target.length - step.revealed
  if (backlog <= 0) return step.target.length
  const base = step.charsPerSecond ?? responseRevealCharsPerSecond
  const maxBacklog = step.maxBacklogChars ?? responseRevealMaxBacklogChars
  const rate = base * Math.min(2, Math.max(1, backlog / maxBacklog))
  const budget = Math.floor((rate * Math.max(0, step.elapsedMs)) / 1000)
  return revealBoundary(step.target, step.revealed, Math.min(step.target.length, step.revealed + budget))
}

export type RevealPacerOptions = {
  schedule: (callback: () => void) => number
  cancel: (handle: number) => void
  now: () => number
  emit: (text: string) => void
  charsPerSecond?: number | (() => number)
  maxBacklogChars?: number
}

/**
 * Paces streamed text into the DOM at a steady rate.
 *
 * Callers push the full text they have received so far; the pacer emits progressively longer
 * prefixes on each frame until it catches up. Text that is not an extension of what is already
 * revealed (a revert, a compaction, or a different message) is emitted immediately instead.
 */
export function createRevealPacer(options: RevealPacerOptions) {
  let target = ""
  let revealed = 0
  let frame: number | undefined
  let last = 0

  const stop = () => {
    if (frame !== undefined) options.cancel(frame)
    frame = undefined
  }

  const start = () => {
    if (frame === undefined) frame = options.schedule(tick)
  }

  function tick() {
    frame = undefined
    const now = options.now()
    const charsPerSecond =
      typeof options.charsPerSecond === "function" ? options.charsPerSecond() : options.charsPerSecond
    const next = revealStep({
      revealed,
      target,
      elapsedMs: now - last,
      charsPerSecond,
      maxBacklogChars: options.maxBacklogChars,
    })
    if (next !== revealed) {
      last = now
      revealed = next
      options.emit(target.slice(0, revealed))
    }
    if (revealed < target.length) start()
  }

  return {
    push(text: string) {
      if (!text.startsWith(target.slice(0, revealed))) revealed = text.length
      target = text
      if (revealed >= target.length) {
        revealed = target.length
        stop()
        options.emit(target)
        return
      }
      if (frame === undefined) last = options.now()
      start()
    },
    /** Reveals everything immediately, used when a response completes or the reader interacts. */
    flush(text?: string) {
      if (text !== undefined) target = text
      stop()
      revealed = target.length
      options.emit(target)
    },
    /** Drops all pacing state so the next push starts a fresh reveal. */
    reset() {
      stop()
      target = ""
      revealed = 0
    },
    dispose: stop,
  }
}

export function interruptResponseAnimations() {
  if (typeof window !== "undefined") window.dispatchEvent(new Event(responseAnimationInterruptEvent))
}
