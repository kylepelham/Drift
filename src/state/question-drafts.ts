import { createSignal } from "solid-js"

export type QuestionDraft = { selected: string[]; custom: string; customSelected: boolean }
export type QuestionDraftState = { step: number; drafts: QuestionDraft[] }

const [states, setStates] = createSignal<Record<string, QuestionDraftState>>({})

function emptyDraft(): QuestionDraft {
  return { selected: [], custom: "", customSelected: false }
}

export function questionDraftState(requestID: string, count: number): QuestionDraftState {
  const current = states()[requestID]
  return {
    step: Math.min(current?.step ?? 0, Math.max(0, count - 1)),
    drafts: Array.from({ length: count }, (_, index) => current?.drafts[index] ?? emptyDraft()),
  }
}

export function setQuestionDraftStep(requestID: string, count: number, step: number) {
  const current = questionDraftState(requestID, count)
  setStates({ ...states(), [requestID]: { ...current, step: Math.max(0, Math.min(step, Math.max(0, count - 1))) } })
}

export function updateQuestionDraft(requestID: string, count: number, index: number, draft: QuestionDraft) {
  const current = questionDraftState(requestID, count)
  setStates({
    ...states(),
    [requestID]: { ...current, drafts: current.drafts.map((item, itemIndex) => (itemIndex === index ? draft : item)) },
  })
}

export function clearQuestionDraft(requestID: string) {
  if (!states()[requestID]) return
  const next = { ...states() }
  delete next[requestID]
  setStates(next)
}
