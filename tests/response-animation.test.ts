import { expect, test } from "bun:test"
import {
  responseBurstSize,
  responseBurstThreshold,
  responseRevealClass,
  responseRevealDuration,
  responseRevealMaximumMs,
  revealResponseNodes,
  type ResponseBurstUpdate,
} from "../src/ui/response-animation"

const liveUpdate: ResponseBurstUpdate = {
  previousLength: 20,
  nextLength: 20 + responseBurstThreshold,
  previousIdentity: "message:part",
  identity: "message:part",
  mounted: true,
  live: true,
  enabled: true,
  reducedMotion: false,
}

test("response bursts animate only for an already-mounted live part", () => {
  expect(responseBurstSize(liveUpdate)).toBe(responseBurstThreshold)
  expect(responseBurstSize({ ...liveUpdate, mounted: false })).toBe(0)
  expect(responseBurstSize({ ...liveUpdate, live: false })).toBe(0)
  expect(responseBurstSize({ ...liveUpdate, previousIdentity: "historical:part" })).toBe(0)
  expect(responseBurstSize({ ...liveUpdate, nextLength: liveUpdate.nextLength - 1 })).toBe(0)
})

test("response bursts stay instant when disabled or reduced motion is requested", () => {
  expect(responseBurstSize({ ...liveUpdate, enabled: false })).toBe(0)
  expect(responseBurstSize({ ...liveUpdate, reducedMotion: true })).toBe(0)
})

test("response reveal duration scales with a hard one-second cap", () => {
  expect(responseRevealDuration(responseBurstThreshold)).toBeGreaterThan(0)
  expect(responseRevealDuration(responseBurstThreshold * 4)).toBeGreaterThan(
    responseRevealDuration(responseBurstThreshold),
  )
  expect(responseRevealDuration(10_000_000)).toBe(responseRevealMaximumMs)
})

test("response reveal keeps full text present and can be interrupted", () => {
  const classes = new Set<string>()
  const styles = new Map<string, string>()
  const node = {
    textContent: "The complete response is already in the DOM.",
    classList: {
      add: (name: string) => classes.add(name),
      remove: (name: string) => classes.delete(name),
    },
    style: {
      setProperty: (name: string, value: string) => styles.set(name, value),
      removeProperty: (name: string) => styles.delete(name),
    },
  } as unknown as HTMLElement

  const finish = revealResponseNodes([node], 500)
  expect(node.textContent).toBe("The complete response is already in the DOM.")
  expect(classes.has(responseRevealClass)).toBeTrue()
  finish()
  expect(classes.has(responseRevealClass)).toBeFalse()
  expect(styles.size).toBe(0)
})

test("response animation preference defaults off", async () => {
  if (!("localStorage" in globalThis))
    Object.defineProperty(globalThis, "localStorage", {
      value: { getItem: () => null, setItem: () => undefined },
    })
  const { animateResponses } = await import("../src/state/prefs")
  expect(animateResponses()).toBeFalse()
})
