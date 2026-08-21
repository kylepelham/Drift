export const responseAnimationInterruptEvent = "drift:response-animation-interrupt"
export const responseRevealMinimumMs = 120
export const responseRevealMaximumMs = 1_000
export const responseRevealMaximumSegments = 240

export function responseBurstSize(previousLength: number, nextLength: number, live: boolean, done: boolean) {
  return live && !done && nextLength > previousLength ? nextLength - previousLength : 0
}

export function responseRevealDuration(characterCount: number, speed: number) {
  if (characterCount <= 0) return 0
  const pacedDuration = (characterCount / Math.max(1, speed)) * 1_000
  return Math.min(responseRevealMaximumMs, Math.max(responseRevealMinimumMs, Math.round(pacedDuration)))
}

export function responseRevealSegmentSize(characterCount: number) {
  return Math.max(1, Math.ceil(characterCount / responseRevealMaximumSegments))
}

export function shouldPreserveResponseReveal(
  active: boolean,
  previousLength: number,
  nextLength: number,
  live: boolean,
  done: boolean,
) {
  if (!active || nextLength < previousLength) return false
  return done || (live && nextLength > previousLength)
}

export function revealResponseNodes(nodes: HTMLElement[], duration: number, onComplete?: () => void) {
  if (!nodes.length || duration <= 0) return () => {}
  const fade = Math.min(90, Math.max(45, Math.round((duration / nodes.length) * 3)))
  const delayRange = Math.max(0, duration - fade)
  for (let index = 0; index < nodes.length; index++) {
    const node = nodes[index]
    const delay = nodes.length === 1 ? 0 : Math.round((delayRange * index) / (nodes.length - 1))
    node.style.setProperty("--response-reveal-duration", `${duration}ms`)
    node.style.setProperty("--response-reveal-fade", `${fade}ms`)
    node.style.setProperty("--response-reveal-delay", `${delay}ms`)
    node.classList.add("md-response-reveal")
  }
  nodes.at(-1)?.classList.add("md-response-reveal-tail")
  let timer: ReturnType<typeof setTimeout> | undefined
  let finished = false
  const finish = (completed = false) => {
    if (finished) return
    finished = true
    if (timer !== undefined) clearTimeout(timer)
    timer = undefined
    for (const node of nodes) {
      node.classList.remove("md-response-reveal", "md-response-reveal-tail")
      node.style.removeProperty("--response-reveal-duration")
      node.style.removeProperty("--response-reveal-fade")
      node.style.removeProperty("--response-reveal-delay")
    }
    if (completed) onComplete?.()
  }
  timer = setTimeout(() => finish(true), duration)
  return () => finish()
}

export function interruptResponseAnimations() {
  if (typeof window !== "undefined") window.dispatchEvent(new Event(responseAnimationInterruptEvent))
}
