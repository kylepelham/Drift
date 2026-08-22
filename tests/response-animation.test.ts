import { expect, test } from "bun:test"
import {
  responseBurstSize,
  responseRevealDuration,
  responseRevealMaximumMs,
  responseRevealMaximumSegments,
  responseRevealMinimumMs,
  responseRevealSegmentSize,
  revealResponseNodes,
  shouldPreserveResponseReveal,
} from "../src/ui/response-animation"
import {
  normalizeResponseAnimationSpeed,
  responseAnimationSpeedDefault,
  responseAnimationSpeedMax,
  responseAnimationSpeedMin,
} from "../src/state/prefs"

test("every new live character is eligible for reveal", () => {
  expect(responseBurstSize(0, 1, true, false)).toBe(1)
  expect(responseBurstSize(100, 101, true, false)).toBe(1)
  expect(responseBurstSize(100, 500, false, false)).toBe(0)
  expect(responseBurstSize(100, 500, true, true)).toBe(0)
  expect(responseBurstSize(500, 100, true, false)).toBe(0)
})

test("reveal duration follows the speed preference within a short bound", () => {
  expect(responseRevealDuration(0, responseAnimationSpeedDefault)).toBe(0)
  expect(responseRevealDuration(1, responseAnimationSpeedDefault)).toBe(responseRevealMinimumMs)
  expect(responseRevealDuration(144, 144)).toBe(responseRevealMaximumMs)
  expect(responseRevealDuration(20_000, responseAnimationSpeedMin)).toBe(responseRevealMaximumMs)
  expect(responseRevealDuration(120, 600)).toBe(200)
})

test("large additions use a bounded number of typing segments", () => {
  expect(responseRevealSegmentSize(1)).toBe(1)
  expect(responseRevealSegmentSize(responseRevealMaximumSegments)).toBe(1)
  expect(responseRevealSegmentSize(responseRevealMaximumSegments + 1)).toBe(2)
  expect(responseRevealSegmentSize(20_000)).toBe(84)
})

test("active response reveals survive new deltas and normal completion", () => {
  expect(shouldPreserveResponseReveal(true, 100, 101, true, false)).toBeTrue()
  expect(shouldPreserveResponseReveal(true, 100, 101, true, true)).toBeTrue()
  expect(shouldPreserveResponseReveal(true, 100, 100, true, true)).toBeTrue()
  expect(shouldPreserveResponseReveal(false, 100, 101, true, false)).toBeFalse()
  expect(shouldPreserveResponseReveal(true, 100, 101, false, false)).toBeFalse()
  expect(shouldPreserveResponseReveal(true, 101, 100, true, true)).toBeFalse()
})

test("response reveal staggers compositor animation state", () => {
  const fakeNode = () => {
    const properties = new Map<string, string>()
    const classes = new Set<string>()
    const node = {
      style: {
        setProperty: (name: string, value: string) => properties.set(name, value),
        removeProperty: (name: string) => properties.delete(name),
      },
      classList: {
        add: (...names: string[]) => names.forEach((name) => classes.add(name)),
        remove: (...names: string[]) => names.forEach((name) => classes.delete(name)),
      },
    } as unknown as HTMLElement
    return { node, properties, classes }
  }
  const nodes = [fakeNode(), fakeNode(), fakeNode()]
  const finish = revealResponseNodes(nodes.map((entry) => entry.node), 200)

  expect(nodes.map((entry) => entry.properties.get("--response-reveal-delay"))).toEqual(["0ms", "55ms", "110ms"])
  expect(nodes[0].properties.get("--response-reveal-fade")).toBe("90ms")
  finish()
  expect(nodes.every((entry) => !entry.classes.has("md-response-reveal"))).toBeTrue()
  expect(nodes.every((entry) => !entry.properties.has("--response-reveal-duration"))).toBeTrue()
})

test("only natural reveal completion advances queued content", async () => {
  const fakeNode = () => {
    const classes = new Set<string>()
    return {
      style: { setProperty: () => {}, removeProperty: () => {} },
      classList: {
        add: (...names: string[]) => names.forEach((name) => classes.add(name)),
        remove: (...names: string[]) => names.forEach((name) => classes.delete(name)),
      },
    } as unknown as HTMLElement
  }
  let completions = 0
  const finish = revealResponseNodes([fakeNode()], 40, () => completions++)
  finish()
  await Bun.sleep(80)
  expect(completions).toBe(0)

  revealResponseNodes([fakeNode()], 40, () => completions++)
  await Bun.sleep(80)
  expect(completions).toBe(1)
})

test("markdown reveal never drives rendering from animation frames", async () => {
  const [animation, markdown, css] = await Promise.all([
    Bun.file("src/ui/response-animation.ts").text(),
    Bun.file("src/ui/markdown.tsx").text(),
    Bun.file("src/styles/app.css").text(),
  ])
  expect(animation).not.toContain("requestAnimationFrame")
  expect(animation).not.toContain("createRevealPacer")
  expect(markdown).not.toContain("requestAnimationFrame")
  expect(markdown).not.toContain("setRevealed")
  // Formatting newlines under tr/thead/tbody must remain text nodes. Animated spans there become
  // anonymous table cells and split a valid three-column table into seven columns in Chromium.
  expect(markdown).toContain("if (node.textContent?.trim()) additions.push(node as Text)")
  expect(css).toContain("@keyframes response-character-reveal")
  expect(css).toContain(".md-response-reveal")
  expect(css).toContain("display: none")
  expect(css).toContain("display: revert")
})

test("history paging never fires from synthetic scroll positions while stuck to the bottom", async () => {
  const source = await Bun.file("src/ui/chat.tsx").text()
  expect(source).toMatch(/loadingOlder \|\| untrack\(stick\)/)
})

test("response animation speed is bounded and defaults safely", () => {
  expect(normalizeResponseAnimationSpeed(undefined)).toBe(responseAnimationSpeedDefault)
  expect(normalizeResponseAnimationSpeed(responseAnimationSpeedMin - 1)).toBe(responseAnimationSpeedMin)
  expect(normalizeResponseAnimationSpeed(responseAnimationSpeedMax + 1)).toBe(responseAnimationSpeedMax)
  expect(normalizeResponseAnimationSpeed(211.6)).toBe(212)
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
