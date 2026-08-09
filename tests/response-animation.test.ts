import { expect, test } from "bun:test"
import {
  createRevealPacer,
  responseRevealCharsPerSecond,
  responseRevealMaxBacklogChars,
  revealBoundary,
  revealStep,
} from "../src/ui/response-animation"
import {
  normalizeResponseAnimationSpeed,
  responseAnimationSpeedDefault,
  responseAnimationSpeedMax,
  responseAnimationSpeedMin,
} from "../src/state/prefs"

const frameMs = 16

/** Drives a pacer with a fake clock and scheduler, returning the text emitted per frame. */
function runPacer(chunks: string[], options: { framesPerChunk?: number } = {}) {
  const framesPerChunk = options.framesPerChunk ?? 40
  let pending: (() => void) | undefined
  let clock = 0
  const emitted: string[] = []
  const pacer = createRevealPacer({
    schedule: (callback) => {
      pending = callback
      return 1
    },
    cancel: () => (pending = undefined),
    now: () => clock,
    emit: (text) => emitted.push(text),
  })
  for (const chunk of chunks) {
    pacer.push(chunk)
    for (let frame = 0; frame < framesPerChunk && pending; frame++) {
      const callback = pending
      pending = undefined
      clock += frameMs
      callback()
    }
  }
  return { emitted, pacer, remaining: () => !!pending }
}

test("reveal advances by characters instead of jumping between words", () => {
  expect(revealBoundary("hello world again", 0, 11)).toBe(11)
  expect(revealBoundary("hello world again", 0, 8)).toBe(8)
  expect(revealBoundary("supercalifragilistic", 0, 6)).toBe(6)
  expect(revealBoundary("short", 0, 99)).toBe(5)
  expect(revealBoundary("😀😀", 0, 1)).toBe(2)
})

test("reveal speed follows elapsed time, not how much text arrived", () => {
  const small = revealStep({ revealed: 0, target: "x".repeat(5000), elapsedMs: frameMs })
  const large = revealStep({ revealed: 0, target: "x".repeat(5000), elapsedMs: frameMs })
  expect(small).toBe(large)
  // Twice the frame time releases roughly twice the characters.
  const doubled = revealStep({ revealed: 0, target: "x".repeat(5000), elapsedMs: frameMs * 2 })
  expect(doubled).toBeGreaterThan(small)
  expect(revealStep({ revealed: 10, target: "0123456789", elapsedMs: frameMs })).toBe(10)
})

test("a large backlog accelerates but still spans multiple frames", () => {
  const backlog = responseRevealMaxBacklogChars * 4
  const target = "x".repeat(backlog)
  const burst = revealStep({ revealed: 0, target, elapsedMs: frameMs })
  const steady = revealStep({ revealed: 0, target: "x".repeat(100), elapsedMs: frameMs })
  expect(burst).toBeGreaterThan(steady)
  expect(burst).toBeLessThanOrEqual(steady * 2)
  expect(burst).toBeLessThan(backlog)
})

test("normal streaming types roughly one character per display frame", () => {
  const target = "a response long enough to animate smoothly"
  const first = revealStep({ revealed: 0, target, elapsedMs: frameMs })
  const second = revealStep({ revealed: first, target, elapsedMs: frameMs })
  expect(first).toBe(2)
  expect(second).toBe(4)
})

test("response animation speed is bounded and defaults safely", () => {
  expect(normalizeResponseAnimationSpeed(undefined)).toBe(responseAnimationSpeedDefault)
  expect(normalizeResponseAnimationSpeed(responseAnimationSpeedMin - 1)).toBe(responseAnimationSpeedMin)
  expect(normalizeResponseAnimationSpeed(responseAnimationSpeedMax + 1)).toBe(responseAnimationSpeedMax)
  expect(normalizeResponseAnimationSpeed(211.6)).toBe(212)
})

test("one large chunk and many small chunks reveal at the same pace", () => {
  const full = "word ".repeat(400).trim()
  const single = runPacer([full])
  const incremental = runPacer(
    Array.from({ length: 40 }, (_, index) => full.slice(0, Math.round((full.length * (index + 1)) / 40))),
    { framesPerChunk: 1 },
  )
  const lengthsAfter = (emitted: string[], frames: number) => emitted[Math.min(frames, emitted.length) - 1]?.length ?? 0
  // Both transports converge on the same revealed length after the same number of frames, which is
  // what makes the reveal look identical for websocket deltas and coarse REST blocks.
  expect(Math.abs(lengthsAfter(single.emitted, 10) - lengthsAfter(incremental.emitted, 10))).toBeLessThan(
    responseRevealCharsPerSecond,
  )
  expect(single.emitted.length).toBeGreaterThan(3)
  expect(incremental.emitted.length).toBeGreaterThan(3)
})

test("the first chunk after mount animates instead of appearing at once", () => {
  const { emitted } = runPacer(["a long first response that arrives as one complete block"], { framesPerChunk: 3 })
  expect(emitted.length).toBeGreaterThan(1)
  expect(emitted[0]!.length).toBeLessThan("a long first response that arrives as one complete block".length)
})

test("rewritten text snaps instead of rewinding the reveal", () => {
  // A revert or compaction replaces the text rather than extending it, so pacing from the old
  // offset would rewind visible content. Those updates appear immediately instead.
  const { emitted } = runPacer(["the original streamed answer", "completely different text"], { framesPerChunk: 2 })
  expect(emitted.at(-1)).toBe("completely different text")
  expect(emitted.length).toBeGreaterThan(1)
})

test("flush reveals everything and stops scheduling frames", () => {
  const run = runPacer(["a streamed response that is still being revealed"], { framesPerChunk: 1 })
  run.pacer.flush()
  expect(run.emitted.at(-1)).toBe("a streamed response that is still being revealed")
  run.pacer.dispose()
})

test("response animation preference defaults off", async () => {
  if (!("localStorage" in globalThis))
    Object.defineProperty(globalThis, "localStorage", {
      value: { getItem: () => null, setItem: () => undefined },
    })
  const { animateResponses, responseAnimationSpeed } = await import("../src/state/prefs")
  expect(animateResponses()).toBeFalse()
  expect(responseAnimationSpeed()).toBe(responseAnimationSpeedDefault)
})
