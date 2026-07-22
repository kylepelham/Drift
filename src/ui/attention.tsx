import type { Permission } from "@opencode-ai/sdk/client"
import { createSignal, For, Show } from "solid-js"
import { useEngine } from "../engine"
import type { PermissionResponse } from "../engine/actions"
import type { QuestionInfo } from "../engine/store"
import { selectedSession } from "../state/selection"
import { Chevron } from "./parts"

export function AttentionStrip() {
  return (
    <div class="mx-auto flex w-full max-w-3xl flex-col gap-2">
      <ErrorBanner />
      <TodoStrip />
    </div>
  )
}

function ErrorBanner() {
  const engine = useEngine()
  const error = () => engine.state.errors[selectedSession() ?? ""]
  return (
    <Show when={error()}>
      <div class="dock-card fade-up rounded-lg border border-danger/40 bg-danger/10 px-3 py-2 text-sm text-danger select-text">{error()}</div>
    </Show>
  )
}

function TodoStrip() {
  const engine = useEngine()
  const [open, setOpen] = createSignal(false)
  const todos = () => engine.state.todos[selectedSession() ?? ""] ?? []
  const remaining = () => todos().filter((todo) => todo.status !== "completed" && todo.status !== "cancelled")
  return (
    <Show when={remaining().length > 0}>
      <div class="dock-card plan-card rounded-lg border border-edge bg-surface text-sm">
        <button
          class="flex w-full items-center gap-2 px-3 py-1.5 text-ink-muted"
          onClick={() => setOpen(!open())}
        >
          <Chevron open={open()} />
          <span>
            Plan · {todos().length - remaining().length}/{todos().length} done
          </span>
          <span class="truncate text-ink-faint">
            {todos().find((todo) => todo.status === "in_progress")?.content}
          </span>
        </button>
        <Show when={open()}>
          <ul class="space-y-1 border-t border-edge px-3 py-2">
            <For each={todos()}>
              {(todo) => (
                <li
                  class="flex items-center gap-2 text-xs"
                  classList={{
                    "text-ink-faint line-through": todo.status === "completed",
                    "text-accent": todo.status === "in_progress",
                    "text-ink-muted": todo.status === "pending",
                  }}
                >
                  <span class="size-1 rounded-full bg-current" />
                  {todo.content}
                </li>
              )}
            </For>
          </ul>
        </Show>
      </div>
    </Show>
  )
}

export function PermissionCard(props: { permission: Permission }) {
  const engine = useEngine()
  const reply = (response: PermissionResponse) =>
    void engine.actions.replyPermission(props.permission.sessionID, props.permission.id, response)
  return (
    <div class="fade-up rounded-lg border border-warn/40 bg-warn/10 px-3 py-2.5">
      <div class="mb-2 text-sm">
        <span class="text-warn">Permission</span> <span class="text-ink">{props.permission.title}</span>
        <Show when={props.permission.pattern}>
          <code class="ml-2 rounded bg-raised px-1.5 py-0.5 font-mono text-xs text-ink-muted">
            {[props.permission.pattern].flat().join(", ")}
          </code>
        </Show>
      </div>
      <div class="flex gap-2">
        <ActionButton label="Allow once" onClick={() => reply("once")} />
        <ActionButton label="Always allow" onClick={() => reply("always")} />
        <ActionButton label="Deny" danger onClick={() => reply("reject")} />
      </div>
    </div>
  )
}

export function QuestionCard(props: { questions: QuestionInfo[]; onAnswer: (answers: string[][] | null) => void }) {
  const [step, setStep] = createSignal(0)
  const [collected, setCollected] = createSignal<string[][]>([])
  const [picked, setPicked] = createSignal<string[]>([])
  const [custom, setCustom] = createSignal("")
  const current = () => props.questions[step()]

  function submitStep(answer: string[]) {
    const answers = [...collected(), answer]
    if (step() + 1 < props.questions.length) {
      setCollected(answers)
      setStep(step() + 1)
      setPicked([])
      setCustom("")
      return
    }
    props.onAnswer(answers)
  }

  function togglePick(label: string) {
    if (!current()?.multiple) return submitStep([label])
    setPicked(picked().includes(label) ? picked().filter((item) => item !== label) : [...picked(), label])
  }

  return (
    <Show when={current()}>
      {(question) => (
        <div class="fade-up rounded-lg border border-accent/40 bg-accent/5 px-3 py-2.5">
          <div class="mb-2 text-sm">
            <span class="text-accent">{question().header}</span>{" "}
            <span class="text-ink">{question().question}</span>
            <Show when={props.questions.length > 1}>
              <span class="ml-2 text-xs text-ink-faint">
                {step() + 1}/{props.questions.length}
              </span>
            </Show>
          </div>
          <div class="flex flex-wrap gap-2">
            <For each={question().options}>
              {(option) => (
                <button
                  class="rounded-md border px-2.5 py-1 text-left text-xs transition-colors"
                  classList={{
                    "border-accent text-ink": picked().includes(option.label),
                    "border-edge text-ink-muted hover:border-edge-strong hover:text-ink": !picked().includes(option.label),
                  }}
                  title={option.description}
                  onClick={() => togglePick(option.label)}
                >
                  {option.label}
                </button>
              )}
            </For>
          </div>
          <Show when={question().custom !== false}>
            <input
              class="mt-2 w-full rounded-md border border-edge bg-surface px-2.5 py-1.5 text-sm outline-none placeholder:text-ink-faint focus:border-edge-strong"
              placeholder="Type your own answer..."
              value={custom()}
              onInput={(event) => setCustom(event.currentTarget.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter" && custom().trim()) submitStep([custom().trim()])
              }}
            />
          </Show>
          <div class="mt-2 flex gap-2">
            <Show when={question().multiple}>
              <ActionButton label="Submit" onClick={() => picked().length > 0 && submitStep(picked())} />
            </Show>
            <ActionButton label="Dismiss" danger onClick={() => props.onAnswer(null)} />
          </div>
        </div>
      )}
    </Show>
  )
}

function ActionButton(props: { label: string; danger?: boolean; onClick: () => void }) {
  return (
    <button
      class="rounded-md border px-2.5 py-1 text-xs transition-colors"
      classList={{
        "border-edge text-ink-muted hover:border-edge-strong hover:text-ink": !props.danger,
        "border-danger/40 text-danger hover:bg-danger/10": props.danger,
      }}
      onClick={props.onClick}
    >
      {props.label}
    </button>
  )
}
