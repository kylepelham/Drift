import type { MessageEntry } from "../engine/store"

type ClarificationItem = { header: string; question: string; answers: string[] }
export type ClarificationAnswer = { items: ClarificationItem[]; text: string; preview: string; spawned?: boolean }

export function clarificationAnswer(entry: MessageEntry): ClarificationAnswer | undefined {
  // Held worker results can ride along with an answer; they are not the user's words.
  const visible = entry.parts.filter((part) => !(part.type === "text" && part.synthetic))
  if (entry.info.role !== "user" || visible.length !== 1) return
  const part = visible[0]
  if (part.type !== "text" || part.synthetic) return
  const metadata = part.metadata?.driftClarification
  if (metadata !== undefined) {
    const items = clarificationItems(metadata)
    if (!items) return

    return {
      items,
      text: items.map((item) => `${item.question}\n${item.answers.join(", ")}`).join("\n\n"),
      preview: items.flatMap((item) => item.answers).join(", "),
    }
  }
  // Earlier builds persisted only this protocol text. Preserve its body without guessing Q&A boundaries.
  const legacy = /^Answer to clarification que_[a-zA-Z0-9]+:\r?\n([\s\S]+)$/.exec(part.text)
  if (legacy) return { items: [], text: legacy[1], preview: "" }
}

function clarificationItems(metadata: unknown): ClarificationItem[] | undefined {
  if (!metadata || typeof metadata !== "object") return

  const data = metadata as Record<string, unknown>
  if (data.version !== 1 || typeof data.requestID !== "string" || !Array.isArray(data.items) || !data.items.length)
    return

  const items: ClarificationItem[] = []
  for (const item of data.items) {
    if (!validClarificationItem(item)) return
    items.push({ header: item.header, question: item.question, answers: [...item.answers] })
  }

  return items
}

function validClarificationItem(item: unknown): item is ClarificationItem {
  if (!item || typeof item !== "object") return false

  const value = item as Record<string, unknown>
  return (
    typeof value.header === "string" &&
    typeof value.question === "string" &&
    Array.isArray(value.answers) &&
    value.answers.every((answer: unknown) => typeof answer === "string")
  )
}
