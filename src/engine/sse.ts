import type { Event } from "@opencode-ai/sdk/client"
import type { EngineTarget } from "./connection"

export const eventInactivityMs = 25_000

export async function streamEvents(
  target: EngineTarget,
  signal: AbortSignal,
  onEvent: (event: Event, directory?: string) => void,
  inactivityMs = eventInactivityMs,
  onExit: () => void = () => undefined,
) {
  const controller = new AbortController()
  let reader: ReadableStreamDefaultReader<Uint8Array> | undefined
  let timer: ReturnType<typeof setTimeout> | undefined
  let inactive = false
  const cancelReader = () => void reader?.cancel().catch(() => undefined)
  const abort = () => {
    controller.abort(signal.reason)
    cancelReader()
  }
  const armWatchdog = () => {
    if (timer) clearTimeout(timer)
    timer = setTimeout(() => {
      inactive = true
      controller.abort("event stream inactive")
      cancelReader()
    }, inactivityMs)
  }
  signal.addEventListener("abort", abort, { once: true })
  if (signal.aborted) abort()
  armWatchdog()
  try {
    const res = await fetch(`${target.url}/global/event`, { headers: target.headers, signal: controller.signal })
    if (!res.ok || !res.body) throw new Error(`event stream ${res.status}`)
    reader = res.body.getReader()
    const decoder = new TextDecoder()
    let buffer = ""
    for (;;) {
      const chunk = await reader.read()
      if (inactive) throw new Error("event stream inactive")
      if (chunk.done) return
      armWatchdog()
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
    signal.removeEventListener("abort", abort)
    controller.abort()
    await reader?.cancel().catch(() => undefined)
    reader?.releaseLock()
    onExit()
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
