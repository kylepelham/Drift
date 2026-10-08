import { selectedSession } from "../state/selection"
import { grantLabel } from "./settings-permissions"
import { createSignal, For, Show } from "solid-js"
import { DiffPanel, parseDiff } from "./parts"
import { RevertDock } from "./revert-dock"
import { TaskDock } from "./task-dock"
import { useEngine } from "../engine"
import { Chevron } from "./controls"
import { IconCheck } from "./icons"
import { t } from "../state/i18n"
import {
  questionDraftState,
  questionSubmissionState,
  setQuestionDraftStep,
  submitQuestionAnswer,
  updateQuestionDraft,
  type QuestionDraft,
} from "../state/question-drafts"

import type { Permission, QuestionInfo } from "../engine/store"
import type { PermissionResponse } from "../engine/actions"
import type { components } from "../engine/native/types"

export function AttentionStrip() {
  return (
    <>
      <TodoStrip />
      <TaskDock />
      <RevertDock />
    </>
  )
}

function TodoStrip() {
  const engine = useEngine()
  const [open, setOpen] = createSignal(false)
  const todos = () => engine.state.todos[selectedSession() ?? ""] ?? []
  const remaining = () => todos().filter((todo) => todo.status !== "completed" && todo.status !== "cancelled")
  return (
    <Show when={remaining().length > 0}>
      <div class="composer-layer-card dock-card rounded-lg border border-edge bg-surface text-sm">
        <button
          class="flex w-full min-w-0 items-center gap-2 overflow-hidden whitespace-nowrap px-3 py-1.5 text-ink-muted"
          onClick={() => setOpen(!open())}
        >
          <Chevron open={open()} />
          <span class="shrink-0">
            {t("session.todo.title")} ·{" "}
            {t("session.todo.progress", { done: todos().length - remaining().length, total: todos().length })}
          </span>
          <span class="min-w-0 flex-1 truncate text-left text-ink-faint">
            {todos().find((todo) => todo.status === "in_progress")?.content}
          </span>
        </button>
        <Show when={open()}>
          <ul class="space-y-0.5 border-t border-edge px-2 py-2">
            <For each={todos()}>
              {(todo) => (
                <li
                  class="flex items-start gap-2 rounded-md px-2 py-1 text-xs"
                  classList={{
                    "text-ink-faint": todo.status === "completed" || todo.status === "cancelled",
                    "bg-accent/5 text-ink": todo.status === "in_progress",
                    "text-ink-muted": todo.status === "pending",
                  }}
                >
                  <span
                    class="mt-px flex size-3.5 shrink-0 items-center justify-center rounded-full border"
                    classList={{
                      "border-edge-strong": todo.status === "completed" || todo.status === "cancelled",
                      "border-accent/60 bg-accent/10 text-accent": todo.status === "in_progress",
                      "border-edge text-ink-faint": todo.status === "pending",
                    }}
                  >
                    <Show when={todo.status === "completed"}>
                      <IconCheck class="size-2.5" />
                    </Show>
                    <Show when={todo.status === "in_progress"}>
                      <span class="size-1.5 rounded-full bg-current" />
                    </Show>
                    <Show when={todo.status === "cancelled"}>
                      <span class="h-px w-1.5 bg-current" />
                    </Show>
                  </span>
                  <span classList={{ "line-through": todo.status === "completed" || todo.status === "cancelled" }}>
                    {todo.content}
                  </span>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </div>
    </Show>
  )
}

type ThreadLink = { label: string; onOpen: () => void }

function ThreadAttribution(props: { thread?: ThreadLink }) {
  return (
    <Show when={props.thread}>
      {(thread) => (
        <button
          class="min-w-0 max-w-48 truncate text-xs text-ink-faint transition-colors hover:text-ink"
          title={t("drift.composer.openThread")}
          onClick={thread().onOpen}
        >
          {thread().label}
        </button>
      )}
    </Show>
  )
}

type AskReason = components["schemas"]["Reason"]

/** Why the engine asks about a shell line no rule decided, as the card says it. */
const REASONS = {
  outside: "drift.permission.reason.outside",
  unresolved: "drift.permission.reason.unresolved",
  secret: "drift.permission.reason.secret",
  searches: "drift.permission.reason.searches",
  beyondUndo: "drift.permission.reason.beyondUndo",
  moves: "drift.permission.reason.moves",
  hidden: "drift.permission.reason.hidden",
} as const satisfies Record<AskReason, string>

export function PermissionCard(props: { permission: Permission; thread?: ThreadLink }) {
  const engine = useEngine()
  const reply = (response: PermissionResponse) =>
    void engine.actions.replyPermission(props.permission.sessionId, props.permission.id, response)
  const diff = () => props.permission.diff
  const filename = () => props.permission.pattern
  const alwaysCovers = () => {
    const grants = props.permission.always ?? []
    return grants.length ? t("drift.permission.alwaysCovers", { what: grants.map(grantLabel).join("; ") }) : undefined
  }
  const reason = () => props.permission.reason
  const target = () => props.permission.pattern
  // A call with no description of its own is titled with its target; that is shown once, below.
  const title = () => (props.permission.title === target() ? "" : props.permission.title)
  return (
    <div class="composer-layer-card fade-up rounded-lg border border-warn/40 bg-surface px-3 py-2.5">
      <div class="mb-2 flex items-start justify-between gap-3">
        <div class="min-w-0 text-sm">
          <span class="text-warn">{t("notification.permission.title")}</span> <span class="text-ink">{title()}</span>
        </div>
        <ThreadAttribution thread={props.thread} />
      </div>
      <Show when={target()}>
        <pre class="mb-2 max-h-32 overflow-auto rounded bg-raised px-2 py-1 font-mono text-xs whitespace-pre-wrap break-all text-ink-muted">
          {target()}
        </pre>
      </Show>
      <Show when={reason()}>{(why) => <div class="mb-2 text-xs text-ink-faint">{t(REASONS[why()])}</div>}</Show>
      <Show when={typeof diff() === "string" && (diff() as string)}>
        {(change) => (
          <div class="mb-2" aria-label={t("drift.permission.change")}>
            <Show
              when={parseDiff(change()).length}
              fallback={
                <pre class="transcript-tool-output max-h-80 overflow-auto rounded-lg border border-edge p-2 font-mono text-xs whitespace-pre text-ink-muted">
                  {change()}
                </pre>
              }
            >
              <DiffPanel diff={change()} filename={filename()} />
            </Show>
          </div>
        )}
      </Show>
      <div class="flex flex-wrap items-center gap-2">
        <ActionButton label={t("settings.permissions.action.allow")} onClick={() => reply("once")} />
        <ActionButton label={t("drift.permission.always")} title={alwaysCovers()} onClick={() => reply("always")} />
        <ActionButton label={t("settings.permissions.action.deny")} danger onClick={() => reply("reject")} />
        <ActionButton
          label={t("drift.permission.stop")}
          title={t("drift.permission.stopHint")}
          danger
          onClick={() => reply("stop")}
        />
      </div>
    </div>
  )
}

export function QuestionCard(props: {
  requestID: string
  async?: boolean
  questions: QuestionInfo[]
  thread?: ThreadLink
  onAnswer: (answers: string[][] | null) => boolean | void | Promise<boolean | void>
}) {
  const [collapsed, setCollapsed] = createSignal(false)
  const submission = () => questionSubmissionState(props.requestID)
  const sending = () => !!submission()?.sending
  const failed = () => !!submission()?.failed
  const locked = () => submission()?.answers !== undefined
  const editingDisabled = () => sending() || locked()
  const hidden = () => !!props.async && collapsed()
  const state = () => questionDraftState(props.requestID, props.questions.length)
  const step = () => state().step
  const drafts = () => state().drafts
  const current = () => props.questions[step()]
  const draft = () => drafts()[step()] ?? { selected: [], custom: "", customSelected: false }

  function update(next: QuestionDraft) {
    if (editingDisabled()) return
    updateQuestionDraft(props.requestID, props.questions.length, step(), next)
  }

  function setStep(next: number) {
    if (sending()) return
    setQuestionDraftStep(props.requestID, props.questions.length, next)
  }

  async function answer(answers: string[][] | null) {
    const requestID = props.requestID
    const completed = await submitQuestionAnswer(requestID, !!props.async, answers, props.onAnswer)
    if (completed === false && props.requestID === requestID) setCollapsed(false)
  }

  function advance() {
    if (sending() || hidden()) return
    const original = submission()?.answers
    if (original) return void answer(original)
    if (!questionAnswer(draft()).length) return
    if (step() + 1 < props.questions.length) return setStep(step() + 1)
    const missing = drafts().findIndex((item) => !questionAnswer(item).length)
    if (missing !== -1) return setStep(missing)
    void answer(drafts().map(questionAnswer))
  }

  return (
    <Show when={current()}>
      {(question) => (
        <div
          class="composer-layer-card fade-up overflow-hidden rounded-xl border border-edge-strong bg-surface shadow-xl shadow-black/15"
          aria-busy={sending()}
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.stopPropagation()
              if (props.async) setCollapsed(true)
              else void answer(null)
            }
            if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
              event.preventDefault()
              advance()
            }
          }}
        >
          <div class="border-b border-edge px-4 py-3.5">
            <div class="flex items-center justify-between gap-3 text-xs font-medium">
              <div class="flex min-w-0 items-center gap-2">
                <span class="shrink-0 text-ink-muted">
                  {t("session.question.progress", { current: step() + 1, total: props.questions.length })}
                </span>
                <ThreadAttribution thread={props.thread} />
              </div>
              <span class="truncate text-accent">{question().header}</span>
            </div>
            <Show when={props.async}>
              <div class="mt-2 flex items-center justify-between gap-3 text-xs">
                <span class="text-ink-faint">{t("drift.question.asyncHint")}</span>
                <button
                  class="shrink-0 rounded px-1 py-0.5 text-accent hover:bg-accent/10"
                  aria-expanded={!hidden()}
                  aria-controls={`question-body-${props.requestID}`}
                  onClick={() => setCollapsed(!hidden())}
                >
                  {hidden() ? t("drift.question.answerNow") : t("drift.question.answerLater")}
                </button>
              </div>
            </Show>
            <Show when={!hidden() && props.questions.length > 1}>
              <div class="mt-3 flex gap-1.5">
                <For each={props.questions}>
                  {(_, index) => (
                    <button
                      class="h-1 flex-1 rounded-full transition-colors"
                      classList={{
                        "bg-accent": index() === step(),
                        "bg-accent/35": index() !== step() && questionAnswer(drafts()[index()]).length > 0,
                        "bg-edge": index() !== step() && questionAnswer(drafts()[index()]).length === 0,
                      }}
                      title={t("drift.question.number", { number: index() + 1 })}
                      disabled={sending()}
                      onClick={() => setStep(index())}
                    />
                  )}
                </For>
              </div>
            </Show>
          </div>
          <fieldset id={`question-body-${props.requestID}`} class="min-w-0" hidden={hidden()} disabled={sending()}>
            <fieldset class="min-w-0 px-4 pt-3.5 pb-4" disabled={editingDisabled()}>
              <div class="text-sm font-medium text-ink">{question().question}</div>
              <div class="mt-1 text-xs text-ink-faint">
                {question().multiple ? t("drift.question.selectMultiple") : t("drift.question.selectOne")}
              </div>
              <div class="question-options mt-3 max-h-[min(26rem,52vh)] space-y-2 overflow-y-auto pr-1">
                <For each={question().options}>
                  {(option) => {
                    const selected = () => draft().selected.includes(option.label)
                    return (
                      <button
                        class="flex w-full items-start gap-3 rounded-lg border px-3.5 py-3 text-left transition-colors"
                        classList={{
                          "border-accent/70 bg-accent/8": selected(),
                          "border-edge bg-raised/30 hover:border-edge-strong hover:bg-raised/60": !selected(),
                        }}
                        role={question().multiple ? "checkbox" : "radio"}
                        aria-checked={selected()}
                        onClick={() => update(selectQuestionOption(draft(), option.label, !!question().multiple))}
                      >
                        <ChoiceMark checked={selected()} multiple={!!question().multiple} />
                        <span class="min-w-0">
                          <span class="block text-sm font-medium text-ink">{option.label}</span>
                          <Show when={option.description}>
                            <span class="mt-0.5 block text-xs leading-relaxed text-ink-muted">
                              {option.description}
                            </span>
                          </Show>
                        </span>
                      </button>
                    )
                  }}
                </For>
                <Show when={question().custom !== false}>
                  <div
                    class="flex w-full cursor-pointer items-start gap-3 rounded-lg border px-3.5 py-3 text-left transition-colors"
                    classList={{
                      "border-accent/70 bg-accent/8": draft().customSelected,
                      "border-edge bg-raised/30 hover:border-edge-strong hover:bg-raised/60": !draft().customSelected,
                    }}
                    role={question().multiple ? "checkbox" : "radio"}
                    aria-checked={draft().customSelected}
                    aria-disabled={editingDisabled()}
                    tabIndex={editingDisabled() ? -1 : 0}
                    onClick={(event) => {
                      if (editingDisabled() || event.target instanceof HTMLInputElement) return
                      const row = event.currentTarget
                      update(selectQuestionCustom(draft(), !!question().multiple))
                      queueMicrotask(() => row.querySelector("input")?.focus())
                    }}
                    onKeyDown={(event) => {
                      if (event.key === "Enter" || event.key === " ") {
                        event.preventDefault()
                        update(selectQuestionCustom(draft(), !!question().multiple))
                      }
                    }}
                  >
                    <ChoiceMark checked={draft().customSelected} multiple={!!question().multiple} />
                    <div class="min-w-0 flex-1">
                      <div class="text-sm font-medium text-ink">{t("drift.question.custom")}</div>
                      <Show
                        when={draft().customSelected}
                        fallback={<div class="mt-0.5 text-xs text-ink-muted">{t("drift.question.customHint")}</div>}
                      >
                        <input
                          class="mt-2 w-full rounded-md border border-edge bg-surface px-2.5 py-2 text-sm text-ink outline-none placeholder:text-ink-faint focus:border-accent/70"
                          placeholder={t("drift.question.customPlaceholder")}
                          value={draft().custom}
                          onClick={(event) => event.stopPropagation()}
                          onInput={(event) =>
                            update({ ...draft(), custom: event.currentTarget.value, customSelected: true })
                          }
                          onKeyDown={(event) => {
                            if (event.key === "Escape") return
                            event.stopPropagation()
                            if (event.key === "Enter" && !event.shiftKey) {
                              event.preventDefault()
                              advance()
                            }
                          }}
                        />
                      </Show>
                    </div>
                  </div>
                </Show>
              </div>
            </fieldset>
            <Show when={locked()}>
              <div role={failed() ? "alert" : "status"} class="px-4 pb-3 text-xs text-ink-muted">
                {t("session.question.deliveryUnconfirmed")}
              </div>
            </Show>
            <Show when={failed() && !locked()}>
              <div role="alert" class="px-4 pb-3 text-xs text-danger">
                {t("drift.question.sendFailed")}
              </div>
            </Show>
            <div class="flex flex-wrap items-center justify-between gap-2 border-t border-edge bg-raised/20 px-4 py-3">
              <ActionButton label={t("common.dismiss")} danger onClick={() => void answer(null)} />
              <div class="flex flex-wrap gap-2">
                <Show when={step() > 0}>
                  <ActionButton label={t("common.goBack")} onClick={() => setStep(step() - 1)} />
                </Show>
                <button
                  class="rounded-md border border-accent/60 bg-accent/15 px-3 py-1.5 text-xs font-medium text-ink transition-colors hover:bg-accent/25 disabled:opacity-50"
                  disabled={sending() || (!locked() && !questionAnswer(draft()).length)}
                  onClick={advance}
                >
                  {t(questionSubmitLabel(sending(), locked(), step() + 1 < props.questions.length))}
                </button>
              </div>
            </div>
          </fieldset>
        </div>
      )}
    </Show>
  )
}

export function selectQuestionOption(draft: QuestionDraft, label: string, multiple: boolean): QuestionDraft {
  if (!multiple) return { ...draft, selected: [label], customSelected: false }
  const selected = draft.selected.includes(label)
    ? draft.selected.filter((item) => item !== label)
    : [...draft.selected, label]
  return { ...draft, selected }
}

export function selectQuestionCustom(draft: QuestionDraft, multiple: boolean): QuestionDraft {
  return { ...draft, selected: multiple ? draft.selected : [], customSelected: true }
}

export function questionAnswer(draft: QuestionDraft) {
  const custom = draft.custom.trim()
  return [...draft.selected, ...(draft.customSelected && custom ? [custom] : [])]
}

function questionSubmitLabel(sending: boolean, locked: boolean, hasNext: boolean) {
  if (sending) return "drift.question.sending"
  if (locked) return "session.question.retryOriginal"
  if (hasNext) return "dialog.releaseNotes.action.next"

  return "common.submit"
}

function ChoiceMark(props: { checked: boolean; multiple: boolean }) {
  return (
    <span
      class="mt-0.5 flex size-4 shrink-0 items-center justify-center border transition-colors"
      classList={{
        "rounded-[4px]": props.multiple,
        "rounded-full": !props.multiple,
        "border-accent bg-accent text-white": props.checked,
        "border-edge-strong bg-surface": !props.checked,
      }}
    >
      <Show when={props.checked}>
        <Show when={props.multiple} fallback={<span class="size-1.5 rounded-full bg-white" />}>
          <IconCheck class="size-3" />
        </Show>
      </Show>
    </span>
  )
}

function ActionButton(props: { label: string; danger?: boolean; title?: string; onClick: () => void }) {
  return (
    <button
      title={props.title}
      class="rounded-md border px-2.5 py-1 text-xs transition-colors"
      classList={{
        "border-edge text-ink-muted hover:border-edge-strong hover:text-ink": !props.danger,
        "border-danger/40 text-danger hover:bg-danger/10": props.danger,
      }}
      onClick={() => props.onClick()}
    >
      {props.label}
    </button>
  )
}
