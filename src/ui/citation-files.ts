import type { EngineState, MessageEntry } from "../engine/store"
import { classifyMarkdownLink } from "./markdown-links"

/** Build context only on a click, and stop at the cited message/part so old links stay stable. */
export function citationFileGroups(
  state: Pick<EngineState, "sessions" | "transcripts">,
  sessionID: string,
  messageID?: string,
  partID?: string,
  beforeTime?: number,
): string[][] {
  const directory = state.sessions[sessionID]?.directory
  const entries = (state.transcripts[sessionID] ?? []).filter((entry) =>
    entry.info.sessionID === sessionID && (beforeTime === undefined || entry.info.time.created <= beforeTime))
  const end = messageID ? entries.findIndex((entry) => entry.info.id === messageID) : entries.length - 1
  if (!directory || end < 0) return []
  if (partID && !entries[end].parts.some((part) => part.id === partID)) return []
  let start = end
  while (start > 0 && entries[start].info.role !== "user") start--
  return [collect(entries.slice(start, end + 1)), collect(entries.slice(0, start))]

  function collect(messages: MessageEntry[]) {
    const files = new Set<string>()
    function add(value: unknown) {
      if (typeof value !== "string") return
      // Tool paths are native strings, not percent-encoded hrefs.
      const href = value.split(/([/\\])/).map((segment) => segment === "/" || segment === "\\" ? segment : encodeURIComponent(segment)).join("")
        .replace(/^([a-z])%3A([/\\])/i, "$1:$2")
      const link = classifyMarkdownLink(href, directory)
      if (link.kind === "file") files.add(link.path)
    }
    for (const entry of messages) {
      for (const part of entry.parts) {
        if (part.sessionID !== sessionID) continue
        if (part.type === "file" && /^file:\/\//i.test(part.url)) {
          const link = classifyMarkdownLink(part.url)
          if (link.kind === "file") files.add(link.path)
        }
        if (part.type === "tool" && part.state.status === "completed" &&
          (beforeTime === undefined || part.state.time.end <= beforeTime)) {
          const input = part.state.input
          const metadata = part.state.metadata
          if (["read", "write", "edit", "multiedit"].includes(part.tool)) add(input.filePath)
          if (part.tool === "edit") add((metadata?.filediff as { file?: string } | undefined)?.file)
          if (part.tool === "apply_patch") {
            if (Array.isArray(metadata?.files)) {
              for (const file of metadata.files) {
                if (!file || typeof file !== "object" || file.type === "delete") continue
                add(file.movePath ?? file.filePath)
              }
            } else if (typeof input.patchText === "string") {
              for (const match of input.patchText.matchAll(/^\*\*\* (?:Add File|Update File|Move to): (.+)$/gm)) add(match[1].trim())
            }
          }
        }
        if (entry.info.id === messageID && part.id === partID) break
      }
    }
    return [...files]
  }
}
