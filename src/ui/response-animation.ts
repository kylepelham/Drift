export const responseBurstThreshold = 80
export const responseRevealMaximumMs = 1000
export const responseRevealClass = "md-response-reveal"
export const responseAnimationInterruptEvent = "drift:response-animation-interrupt"

export type ResponseBurstUpdate = {
  previousLength: number
  nextLength: number
  previousIdentity?: string
  identity: string
  mounted: boolean
  live: boolean
  enabled: boolean
  reducedMotion: boolean
}

export function responseBurstSize(update: ResponseBurstUpdate) {
  const growth = update.nextLength - update.previousLength
  if (
    !update.mounted ||
    !update.live ||
    !update.enabled ||
    update.reducedMotion ||
    update.previousIdentity !== update.identity ||
    growth < responseBurstThreshold
  )
    return 0
  return growth
}

export function responseRevealDuration(burstSize: number) {
  if (burstSize <= 0) return 0
  return Math.min(responseRevealMaximumMs, Math.round(180 + Math.sqrt(burstSize) * 12))
}

export function revealResponseNodes(nodes: HTMLElement[], duration: number) {
  if (!nodes.length || duration <= 0) return () => {}
  for (const node of nodes) {
    node.style.setProperty("--response-reveal-duration", `${duration}ms`)
    node.classList.add(responseRevealClass)
  }
  let timer: ReturnType<typeof setTimeout> | undefined
  const finish = () => {
    if (timer === undefined) return
    clearTimeout(timer)
    timer = undefined
    for (const node of nodes) {
      node.classList.remove(responseRevealClass)
      node.style.removeProperty("--response-reveal-duration")
    }
  }
  timer = setTimeout(finish, duration)
  return finish
}

export function interruptResponseAnimations() {
  if (typeof window !== "undefined") window.dispatchEvent(new Event(responseAnimationInterruptEvent))
}
