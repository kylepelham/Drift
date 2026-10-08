import type { components } from "./native/types"

export type QuestionInfo = {
  question: string
  header: string
  options: { label: string; description: string }[]
  multiple?: boolean
  custom?: boolean
}

/** A native request with display defaults and its owning directory. */
export type QuestionRequest = Omit<components["schemas"]["QuestionRequest"], "questions"> & {
  questions: QuestionInfo[]
  directory?: string
}

export function questionForCard(request: components["schemas"]["QuestionRequest"]): QuestionRequest {
  const questions = request.questions.map((question) => ({
    ...question,
    header: question.header ?? "",
    options: (question.options ?? []).map((option) => ({ ...option, description: option.description ?? "" })),
    multiple: question.multiple ?? false,
    custom: question.custom ?? true,
  }))

  return { ...request, questions, async: request.async ?? false }
}
