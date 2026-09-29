// The event socket: tracks the last applied seq, resumes from it, and asks for a hydrate when it cannot.
import type { Envelope, Frame, Target } from "./client"

export type EventHandlers = {
  /** Replace local state from HTTP. Events arriving meanwhile are held and applied after it resolves. */
  hydrate(seq: number): Promise<void> | void
  event(envelope: Envelope): void
  online?(connected: boolean): void
}

export type EventStream = { close(): void; cursor(): number | undefined }

const initialBackoffMs = 500
const maxBackoffMs = 10_000
const hydrateRetryMs = 1_000

export function connectEvents(target: Target, handlers: EventHandlers): EventStream {
  let cursor: number | undefined
  let instance: string | undefined
  let socket: WebSocket | undefined
  let closed = false
  let backoff = initialBackoffMs
  let retry: ReturnType<typeof setTimeout> | undefined
  let held: Envelope[] | undefined
  let hydrating: Promise<void> | undefined
  let wanted: number | undefined
  // Bumped when the engine instance changes; a hydrate from an older generation must not land.
  let generation = 0

  const applyEvent = (envelope: Envelope) => {
    if (cursor !== undefined && envelope.seq <= cursor) return
    cursor = envelope.seq
    handlers.event(envelope)
  }

  /**
   * Hydrates from `seq`. A request that lands while one is running is folded into the same held
   * buffer and run afterwards; a failed hydrate keeps the cursor where it was and tries again.
   */
  const hydrate = (seq: number) => {
    wanted = seq
    if (hydrating) return
    held ??= []
    const mine = generation
    const live = () => !closed && mine === generation
    hydrating = (async () => {
      while (live() && wanted !== undefined) {
        const target: number = wanted
        wanted = undefined
        try {
          await handlers.hydrate(target)
          if (live()) cursor = target
        } catch {
          if (!live()) break
          await new Promise((resolve) => setTimeout(resolve, hydrateRetryMs))
          wanted ??= target
        }
      }
      if (!live()) return
      const pending = held ?? []
      held = undefined
      hydrating = undefined
      for (const envelope of pending) applyEvent(envelope)
    })()
  }

  /** A different engine instance means nothing held or in flight from the old one may apply. */
  const restart = () => {
    generation += 1
    held = undefined
    hydrating = undefined
    wanted = undefined
    cursor = undefined
  }

  const apply = (frame: Frame) => {
    if (frame.type === "hello") {
      const changed = instance !== undefined && frame.instance !== instance
      if (changed) restart()
      const fresh = cursor === undefined || changed
      instance = frame.instance
      if (fresh) hydrate(frame.seq)
      return
    }
    if (frame.type === "resync") {
      hydrate(frame.seq)
      return
    }
    if (held) held.push(frame)
    else applyEvent(frame)
  }

  const open = () => {
    const base = target.url.replace(/^http/, "ws")
    const resume = cursor === undefined ? "" : `&cursor=${cursor}`
    socket = new WebSocket(`${base}/events?token=${target.token}${resume}`)
    socket.onopen = () => {
      backoff = initialBackoffMs
      handlers.online?.(true)
    }
    socket.onmessage = (message) => apply(JSON.parse(String(message.data)) as Frame)
    socket.onclose = () => {
      handlers.online?.(false)
      if (closed) return
      retry = setTimeout(open, backoff)
      backoff = Math.min(backoff * 2, maxBackoffMs)
    }
  }

  open()
  return {
    close() {
      closed = true
      restart()
      if (retry) clearTimeout(retry)
      socket?.close()
    },
    cursor: () => cursor,
  }
}
