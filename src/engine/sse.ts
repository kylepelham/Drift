import type { Event } from "@opencode-ai/sdk/client"
import type { EngineTarget } from "./connection"

// The engine heartbeats every 10s, so silence means the socket is half-open (proxy, VPN,
// sleep/resume) and reader.read() would park forever, leaving Drift falsely online on stale
// state. The margin absorbs scheduler jitter and proxy buffering without hiding a dead link.
export const eventInactivityMs = 25_000

export async function streamEvents(
  target: EngineTarget,
  signal: AbortSignal,
  onEvent: (event: Event, directory?: string) => void,
  inactivityMs = eventInactivityMs,
) {
  const controller = new AbortController()
  let reader: ReadableStreamDefaultReader<Uint8Array> | undefined
  let timer: ReturnType<typeof setTimeout> | undefined
  let inactive = false
  const cancelReader = () => void reader?.cancel().catch(() => undefined)
  const onAbort = () => {
    controller.abort(signal.reason)
    cancelReader()
  }
  // Any frame rearms the deadline, heartbeats included: this watches the socket, not payloads.
  const arm = () => {
    if (timer) clearTimeout(timer)
    timer = setTimeout(() => {
      inactive = true
      controller.abort("event stream inactive")
      cancelReader()
    }, inactivityMs)
  }
  signal.addEventListener("abort", onAbort, { once: true })
  if (signal.aborted) onAbort()
  arm()
  try {
    const res = await fetch(`${target.url}/global/event`, { headers: target.headers, signal: controller.signal })
    if (!res.ok || !res.body) throw new Error(`event stream ${res.status}`)
    reader = res.body.getReader()
    const decoder = new TextDecoder()
    let buffer = ""
    for (;;) {
      const chunk = await reader.read()
      // Cancelling resolves the pending read as done; throw so the reconnect loop runs.
      if (inactive) throw new Error("event stream inactive")
      if (chunk.done) return
      arm()
      buffer += decoder.decode(chunk.value, { stream: true })
      const lines = buffer.split("\n")
      buffer = lines.pop() ?? ""
      for (const line of lines) {
        const event = parseLine(line)
        if (event) onEvent(event.payload, event.directory)
      }
    }
  } finally {
    if (timer) clearTimeout(timer)
    timer = undefined
    signal.removeEventListener("abort", onAbort)
    controller.abort()
    await reader?.cancel().catch(() => undefined)
  }
}

// Global stream wraps each instance event as { directory, payload }; sync/heartbeat frames are noise here.
function parseLine(line: string): { payload: Event; directory?: string } | undefined {
  if (!line.startsWith("data: ")) return
  try {
    const wrapper = JSON.parse(line.slice(6)) as { directory?: string; payload?: Event }
    const event = wrapper.payload
    if (!event || (event.type as string) === "sync" || (event.type as string) === "server.heartbeat") return
    return { payload: event, directory: wrapper.directory }
  } catch {
    return
  }
}
