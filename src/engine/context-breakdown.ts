import type { Part } from "./shapes"
import type { MessageEntry } from "./store"

export type BreakdownKey = "system" | "user" | "assistant" | "tool"
export type BreakdownSegment = { key: BreakdownKey; tokens: number }

// Rough chars-per-token ratio, the same heuristic upstream's context tab uses.
const charsPerToken = 4
// Upstream's allowance per tool-call argument, since inputs are not re-serialized here.
const charsPerToolArgument = 16

function userChars(part: Part) {
  if (part.type === "text") return part.text.length
  if (part.type === "file") return part.source?.text.value.length ?? 0
  if (part.type === "agent") return part.source?.value.length ?? 0
  return 0
}

function assistantChars(part: Part) {
  if (part.type === "text" || part.type === "reasoning") return { assistant: part.text.length, tool: 0 }
  if (part.type !== "tool") return { assistant: 0, tool: 0 }
  const input = Object.keys(part.state.input ?? {}).length * charsPerToolArgument
  if (part.state.status === "completed") return { assistant: 0, tool: input + part.state.output.length }
  if (part.state.status === "error") return { assistant: 0, tool: input + part.state.error.length }
  return { assistant: 0, tool: input }
}

/** Only messages since the latest compaction summary are still in the model's context. */
function liveEntries(entries: MessageEntry[]) {
  for (let index = entries.length - 1; index >= 0; index--) {
    const info = entries[index]!.info
    if (info.role === "assistant" && info.summary) return entries.slice(index)
  }
  return entries
}

function characterCounts(entries: MessageEntry[]) {
  const counts = { user: 0, assistant: 0, tool: 0 }
  for (const entry of liveEntries(entries)) {
    for (const part of entry.parts) {
      if (entry.info.role === "user") counts.user += userChars(part)
      else {
        const next = assistantChars(part)
        counts.assistant += next.assistant
        counts.tool += next.tool
      }
    }
  }
  return counts
}

// Transcript categories are estimates; whatever they leave unexplained is system prompt plus tool schemas.
export function estimateContextBreakdown(entries: MessageEntry[], total: number): BreakdownSegment[] {
  if (total <= 0) return []
  const chars = characterCounts(entries)
  const estimated = {
    user: Math.ceil(chars.user / charsPerToken),
    assistant: Math.ceil(chars.assistant / charsPerToken),
    tool: Math.ceil(chars.tool / charsPerToken),
  }
  const sum = estimated.user + estimated.assistant + estimated.tool
  const scale = sum > total ? total / sum : 1
  const user = Math.floor(estimated.user * scale)
  const assistant = Math.floor(estimated.assistant * scale)
  const tool = Math.floor(estimated.tool * scale)
  const segments: BreakdownSegment[] = [
    { key: "system", tokens: Math.max(0, total - user - assistant - tool) },
    { key: "user", tokens: user },
    { key: "assistant", tokens: assistant },
    { key: "tool", tokens: tool },
  ]
  return segments.filter((segment) => segment.tokens > 0)
}
