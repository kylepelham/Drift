import { createSignal } from "solid-js"

export type QuestionDraft = { selected: string[]; custom: string; customSelected: boolean }
export type QuestionDraftState = { step: number; drafts: QuestionDraft[] }
export type QuestionSubmissionState = { sending: boolean; failed: boolean; answers?: string[][] }

const [states, setStates] = createSignal<Record<string, QuestionDraftState>>({})
const [submissions, setSubmissions] = createSignal<Record<string, QuestionSubmissionState>>({})

function emptyDraft(): QuestionDraft {
  return { selected: [], custom: "", customSelected: false }
}

export function questionDraftState(requestID: string, count: number): QuestionDraftState {
  const current = states()[requestID]
  return {
    step: Math.min(current?.step ?? 0, Math.max(0, count - 1)),
    drafts: Array.from({ length: count }, (_, index) => {
      const draft = current?.drafts[index] ?? emptyDraft()
      return { ...draft, selected: [...draft.selected] }
    }),
  }
}

export function setQuestionDraftStep(requestID: string, count: number, step: number) {
  const current = questionDraftState(requestID, count)
  setStates({ ...states(), [requestID]: { ...current, step: Math.max(0, Math.min(step, Math.max(0, count - 1))) } })
}

export function updateQuestionDraft(requestID: string, count: number, index: number, draft: QuestionDraft) {
  const submission = submissions()[requestID]
  if (submission?.sending || submission?.answers) return
  const current = questionDraftState(requestID, count)
  setStates({
    ...states(),
    [requestID]: {
      ...current,
      drafts: current.drafts.map((item, itemIndex) =>
        itemIndex === index ? { ...draft, selected: [...draft.selected] } : item,
      ),
    },
  })
}

export function questionSubmissionState(requestID: string): QuestionSubmissionState | undefined {
  const current = submissions()[requestID]
  return current && { ...current, answers: current.answers?.map((row) => [...row]) }
}

export async function submitQuestionAnswer(
  requestID: string,
  async: boolean,
  answers: string[][] | null,
  onAnswer: (answers: string[][] | null) => boolean | void | Promise<boolean | void>,
) {
  const current = submissions()[requestID]
  if (current?.sending) return
  const attempt: QuestionSubmissionState = {
    sending: true,
    failed: false,
    answers: current?.answers ?? (async && answers !== null ? answers.map((row) => [...row]) : undefined),
  }
  setSubmissions((states) => ({ ...states, [requestID]: attempt }))
  let completed: boolean | void = false
  try {
    // Dismissal stays explicit; only an answer retry uses the frozen snapshot.
    completed = await onAnswer(answers === null ? null : (attempt.answers ?? answers).map((row) => [...row]))
  } catch {
    completed = false
  }
  // A confirmed event may clear this request before transport finishes, or a new attempt may replace it.
  if (submissions()[requestID] !== attempt) return
  if (completed !== false) {
    clearQuestionDraft(requestID)
    return true
  }
  setSubmissions((states) => ({ ...states, [requestID]: { ...attempt, sending: false, failed: true } }))
  return false
}

export function clearQuestionDraft(requestID: string) {
  setStates((states) => {
    if (!states[requestID]) return states
    const next = { ...states }
    delete next[requestID]
    return next
  })
  setSubmissions((states) => {
    if (!states[requestID]) return states
    const next = { ...states }
    delete next[requestID]
    return next
  })
}
