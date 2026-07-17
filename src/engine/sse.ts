import type { Event } from "@opencode-ai/sdk/client"
import type { EngineTarget } from "./connection"

export async function streamEvents(
  target: EngineTarget,
  directory: string,
  signal: AbortSignal,
  onEvent: (event: Event) => void,
) {
  const url = new URL(`${target.url}/event`)
  url.searchParams.set("directory", directory)
  const res = await fetch(url, { headers: target.headers, signal })
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

function parseLine(line: string): Event | undefined {
  if (!line.startsWith("data: ")) return
  try {
    return JSON.parse(line.slice(6)) as Event
  } catch {
    return
  }
}
