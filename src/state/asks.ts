import { createSignal } from "solid-js"
import type { QuestionInfo } from "../engine/store"
import { clearQuestionDraft } from "./question-drafts"

export type LocalAsk = {
  id: string
  sessionID: string | null
  questions: QuestionInfo[]
}

const [localAsks, setLocalAsks] = createSignal<LocalAsk[]>([])
const resolvers = new Map<string, (answers: string[][] | null) => void>()

export { localAsks }

export function pushAsk(questions: QuestionInfo[], sessionID: string | null = null) {
  const id = `ask_${crypto.randomUUID()}`
  return new Promise<string[][] | null>((resolve) => {
    resolvers.set(id, resolve)
    setLocalAsks([...localAsks(), { id, sessionID, questions }])
  })
}

export function resolveAsk(id: string, answers: string[][] | null) {
  resolvers.get(id)?.(answers)
  resolvers.delete(id)
  clearQuestionDraft(id)
  setLocalAsks(localAsks().filter((ask) => ask.id !== id))
}

export function clearAsks() {
  for (const ask of localAsks()) {
    resolvers.get(ask.id)?.(null)
    clearQuestionDraft(ask.id)
  }
  resolvers.clear()
  setLocalAsks([])
}
