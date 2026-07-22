import type { Event } from "@opencode-ai/sdk/client"
import type { EngineTarget } from "./connection"

export async function streamEvents(target: EngineTarget, signal: AbortSignal, onEvent: (event: Event) => void) {
  const res = await fetch(`${target.url}/global/event`, { headers: target.headers, signal })
  if (!res.ok || !res.body) throw new Error(`event stream ${res.status}`)
  const reader = res.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ""
  for (;;) {
    const chunk = await reader.read()
    if (chunk.done) return
    buffer += decoder.decode(chunk.value, { stream: true })
    const lines = buffer.split("\n")
    buffer = lines.pop() ?? ""
    for (const line of lines) {
      const event = parseLine(line)
      if (event) onEvent(event)
    }
  }
}

// Global stream wraps each instance event as { directory, payload }; sync/heartbeat frames are noise here.
function parseLine(line: string): Event | undefined {
  if (!line.startsWith("data: ")) return
  try {
    const wrapper = JSON.parse(line.slice(6)) as { payload?: Event }
    const event = wrapper.payload
    if (!event || (event.type as string) === "sync" || (event.type as string) === "server.heartbeat") return
    return event
  } catch {
    return
  }
}
