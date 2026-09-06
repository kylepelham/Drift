import type { MessageEntry } from "../engine/store"

type ClarificationItem = { header: string; question: string; answers: string[] }
type ClarificationAnswer = { items: ClarificationItem[]; text: string; preview: string }

export function clarificationAnswer(entry: MessageEntry): ClarificationAnswer | undefined {
  if (entry.info.role !== "user" || entry.parts.length !== 1) return
  const part = entry.parts[0]
  if (part.type !== "text" || part.synthetic) return
  const metadata = part.metadata?.driftClarification
  if (metadata !== undefined) {
    if (!metadata || typeof metadata !== "object") return
    const data = metadata as Record<string, unknown>
    if (data.version !== 1 || typeof data.requestID !== "string" || !Array.isArray(data.items) || !data.items.length) return
    const items: ClarificationItem[] = []
    for (const item of data.items) {
      if (!item || typeof item !== "object" || typeof item.header !== "string" || typeof item.question !== "string") return
      if (!Array.isArray(item.answers) || !item.answers.every((answer: unknown) => typeof answer === "string")) return
      items.push({ header: item.header, question: item.question, answers: [...item.answers] })
    }
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
