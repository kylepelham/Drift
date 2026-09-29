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

export function connectEvents(target: Target, handlers: EventHandlers): EventStream {
  let cursor: number | undefined
  let instance: string | undefined
  let socket: WebSocket | undefined
  let closed = false
  let backoff = initialBackoffMs
  let retry: ReturnType<typeof setTimeout> | undefined
  let held: Envelope[] | undefined

  const applyEvent = (envelope: Envelope) => {
    if (cursor !== undefined && envelope.seq <= cursor) return
    cursor = envelope.seq
    handlers.event(envelope)
  }

  const hydrate = async (seq: number) => {
    held = []
    try {
      await handlers.hydrate(seq)
    } finally {
      const pending = held
      held = undefined
      cursor = seq
      for (const envelope of pending) applyEvent(envelope)
    }
  }

  const apply = (frame: Frame) => {
    if (frame.type === "hello") {
      const fresh = cursor === undefined || frame.instance !== instance
      instance = frame.instance
      if (fresh) void hydrate(frame.seq)
      return
    }
    if (frame.type === "resync") {
      void hydrate(frame.seq)
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
      if (retry) clearTimeout(retry)
      socket?.close()
    },
    cursor: () => cursor,
  }
}
