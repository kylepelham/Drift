import type { Permission } from "@opencode-ai/sdk/client"
import { createSignal, For, Show } from "solid-js"
import { useEngine } from "../engine"
import type { PermissionResponse } from "../engine/actions"
import { selectedSession } from "../state/selection"
import { Chevron } from "./parts"

export function AttentionStrip() {
  return (
    <div class="mx-auto w-full max-w-3xl space-y-2 px-4">
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
      <div class="fade-up rounded-lg border border-danger/40 bg-danger/10 px-3 py-2 text-sm text-danger select-text">{error()}</div>
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
      <div class="fade-up rounded-lg border border-edge bg-surface text-sm">
        <button
          class="flex w-full items-center gap-2 px-3 py-1.5 text-ink-muted"
          onClick={() => setOpen(!open())}
        >
          <Chevron open={open()} />
          <span>
            plan · {todos().length - remaining().length}/{todos().length} done
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
