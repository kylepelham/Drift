import { afterEach, expect, test } from "bun:test"
import {
  clearQuestionDraft,
  questionDraftState,
  questionSubmissionState,
  setQuestionDraftStep,
  submitQuestionAnswer,
  updateQuestionDraft,
} from "../src/state/question-drafts"

const requestID = "submission-test"
const otherID = "submission-other"
const draft = { selected: ["Tests"], custom: "Custom answer", customSelected: true }

afterEach(() => {
  clearQuestionDraft(requestID)
  clearQuestionDraft(otherID)
})

function deferred() {
  let resolve!: (completed: boolean) => void
  let reject!: (reason: Error) => void
  const promise = new Promise<boolean>((done, fail) => {
    resolve = done
    reject = fail
  })
  return { promise, resolve, reject }
}

test("remount-equivalent reads retain sending and failure without allowing duplicate submissions", async () => {
  updateQuestionDraft(requestID, 2, 0, draft)
  updateQuestionDraft(requestID, 2, 1, draft)
  setQuestionDraftStep(requestID, 2, 1)
  const pending = deferred()
  const answers = [["Tests", "Custom answer"], ["Docs"]]
  const submission = submitQuestionAnswer(requestID, true, answers, () => pending.promise)
  const mounted = questionSubmissionState(requestID)
  expect(mounted).toEqual({ sending: true, failed: false, answers })
  expect(questionSubmissionState(otherID)).toBeUndefined()
  expect(questionSubmissionState(requestID)).toEqual(mounted)
  expect(questionDraftState(requestID, 2).step).toBe(1)
  let duplicates = 0
  await submitQuestionAnswer(requestID, true, [["Changed"]], () => { duplicates++ })
  await submitQuestionAnswer(requestID, true, null, () => { duplicates++ })
  expect(duplicates).toBe(0)

  pending.resolve(false)
  expect(await submission).toBe(false)
  expect(questionSubmissionState(requestID)).toEqual({ sending: false, failed: true, answers })
  expect(questionSubmissionState(otherID)).toBeUndefined()
  expect(questionSubmissionState(requestID)?.answers).toEqual(answers)
  expect(questionDraftState(requestID, 2).drafts).toEqual([draft, draft])
})

test.each(["false", "throw", "reject"])("async %s retains the first answer and prevents edits on retry", async (failure) => {
  updateQuestionDraft(requestID, 1, 0, draft)
  const answers = [["Tests", "Custom answer"]]
  expect(await submitQuestionAnswer(requestID, true, answers, () => {
    if (failure === "throw") throw new Error("offline")
    if (failure === "reject") return Promise.reject(new Error("offline"))
    return false
  })).toBe(false)
  updateQuestionDraft(requestID, 1, 0, { ...draft, selected: ["Changed"], custom: "Changed custom" })
  expect(questionDraftState(requestID, 1).drafts[0]).toEqual(draft)
  expect(questionSubmissionState(requestID)).toEqual({ sending: false, failed: true, answers })
  const sent: (string[][] | null)[] = []
  await submitQuestionAnswer(requestID, true, [["Silently changed"]], (payload) => {
    sent.push(payload)
    return false
  })
  expect(sent).toEqual([answers])
  expect(questionSubmissionState(requestID)?.answers).toEqual(answers)
})

test("drafts and original answers cannot be mutated through inputs, reads, or delivery callbacks", async () => {
  const input = { ...draft, selected: [...draft.selected] }
  updateQuestionDraft(requestID, 1, 0, input)
  input.selected[0] = "Changed input"
  input.custom = "Changed custom"
  const read = questionDraftState(requestID, 1)
  read.drafts[0].selected.push("Changed read")
  read.drafts[0].custom = "Changed custom read"
  expect(questionDraftState(requestID, 1).drafts[0]).toEqual(draft)

  const answers = [["Tests", "Custom answer"], ["Docs"]]
  const pending = deferred()
  const submission = submitQuestionAnswer(requestID, true, answers, (payload) => {
    payload![0][0] = "Changed callback"
    payload!.push(["Extra callback row"])
    return pending.promise
  })
  answers[0][0] = "Changed input"
  answers.push(["Extra input row"])
  const mounted = questionSubmissionState(requestID)!
  mounted.answers![0].push("Changed read")
  mounted.answers!.push(["Extra read row"])
  mounted.sending = false
  expect(questionSubmissionState(requestID)).toEqual({
    sending: true, failed: false, answers: [["Tests", "Custom answer"], ["Docs"]],
  })
  updateQuestionDraft(requestID, 1, 0, { ...draft, custom: "Editing while sending" })
  expect(questionDraftState(requestID, 1).drafts[0]).toEqual(draft)
  pending.resolve(false)
  await submission
  const sent: (string[][] | null)[] = []
  await submitQuestionAnswer(requestID, true, answers, (payload) => {
    sent.push(payload)
    return false
  })
  expect(sent).toEqual([[["Tests", "Custom answer"], ["Docs"]]])
})

test.each([true, undefined])("confirmed %s clears drafts and submission without touching other requests", async (completed) => {
  updateQuestionDraft(requestID, 1, 0, draft)
  updateQuestionDraft(otherID, 1, 0, draft)
  await submitQuestionAnswer(requestID, true, [["Tests"]], () => false)
  await submitQuestionAnswer(otherID, true, [["Other"]], () => false)
  const other = questionSubmissionState(otherID)
  expect(await submitQuestionAnswer(requestID, true, [["Changed"]], () => completed)).toBe(true)
  expect(questionSubmissionState(requestID)).toBeUndefined()
  expect(questionDraftState(requestID, 1).drafts[0].selected).toEqual([])
  expect(questionSubmissionState(otherID)).toEqual(other)
  expect(questionDraftState(otherID, 1).drafts[0]).toEqual(draft)
  updateQuestionDraft(requestID, 1, 0, draft)
  expect(questionDraftState(requestID, 1).drafts[0]).toEqual(draft)
})

test.each(["false", "throw", "true"])("late %s after authoritative clear cannot resurrect a request", async (result) => {
  const pending = deferred()
  const submission = submitQuestionAnswer(requestID, true, [["Original"]], () => pending.promise)
  clearQuestionDraft(requestID)
  if (result === "throw") pending.reject(new Error("late failure"))
  else pending.resolve(result === "true")
  expect(await submission).toBeUndefined()
  expect(questionSubmissionState(requestID)).toBeUndefined()
  expect(questionDraftState(requestID, 1).drafts[0].selected).toEqual([])
})

test.each(["false", "throw", "true"])("late %s cannot overwrite newer drafts or submission state", async (result) => {
  const old = deferred()
  const oldSubmission = submitQuestionAnswer(requestID, true, [["Old"]], () => old.promise)
  clearQuestionDraft(requestID)
  updateQuestionDraft(requestID, 1, 0, draft)
  const pending = deferred()
  const newer = submitQuestionAnswer(requestID, true, [["New"]], () => pending.promise)
  if (result === "throw") old.reject(new Error("late failure"))
  else old.resolve(result === "true")
  await oldSubmission
  expect(questionSubmissionState(requestID)).toEqual({ sending: true, failed: false, answers: [["New"]] })
  expect(questionDraftState(requestID, 1).drafts[0]).toEqual(draft)
  pending.resolve(false)
  await newer
  expect(questionSubmissionState(requestID)).toEqual({ sending: false, failed: true, answers: [["New"]] })
})

test("blocking questions still allow editing after failure and retry the edited answer", async () => {
  updateQuestionDraft(requestID, 1, 0, draft)
  const sent: (string[][] | null)[] = []
  const onAnswer = (payload: string[][] | null) => {
    sent.push(payload)
    return false
  }
  await submitQuestionAnswer(requestID, false, [["Original"]], onAnswer)
  expect(questionSubmissionState(requestID)).toEqual({ sending: false, failed: true, answers: undefined })
  updateQuestionDraft(requestID, 1, 0, { ...draft, custom: "Edited" })
  expect(questionDraftState(requestID, 1).drafts[0].custom).toBe("Edited")
  await submitQuestionAnswer(requestID, false, [["Edited"]], onAnswer)
  expect(sent).toEqual([[["Original"]], [["Edited"]]])
})

test("dismissal stays null and a failed dismissal preserves the original answer for answer retry", async () => {
  updateQuestionDraft(requestID, 1, 0, draft)
  const sent: (string[][] | null)[] = []
  const onAnswer = (payload: string[][] | null) => {
    sent.push(payload)
    return false
  }
  await submitQuestionAnswer(requestID, true, null, onAnswer)
  expect(questionSubmissionState(requestID)?.answers).toBeUndefined()
  updateQuestionDraft(requestID, 1, 0, { ...draft, custom: "Still editable" })
  expect(questionDraftState(requestID, 1).drafts[0].custom).toBe("Still editable")
  await submitQuestionAnswer(requestID, true, [["Original"]], onAnswer)
  await submitQuestionAnswer(requestID, true, null, onAnswer)
  expect(questionSubmissionState(requestID)?.answers).toEqual([["Original"]])
  await submitQuestionAnswer(requestID, true, [["Changed"]], onAnswer)
  expect(sent).toEqual([null, [["Original"]], null, [["Original"]]])
  await submitQuestionAnswer(requestID, true, null, (payload) => {
    sent.push(payload)
    return true
  })
  expect(sent.at(-1)).toBeNull()
  expect(questionSubmissionState(requestID)).toBeUndefined()
  expect(questionDraftState(requestID, 1).drafts[0].selected).toEqual([])
})
