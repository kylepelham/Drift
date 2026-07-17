import type { Part, ReasoningPart, ToolPart } from "@opencode-ai/sdk/client"
import { createSignal, Match, Show, Switch } from "solid-js"
import { Markdown } from "./markdown"

export function PartView(props: { part: Part }) {
  return (
    <Switch>
      <Match when={visibleText(props.part)}>{(part) => <Markdown text={part().text} done={!!part().time?.end} />}</Match>
      <Match when={props.part.type === "reasoning" && (props.part as ReasoningPart)}>
        {(part) => <ReasoningView part={part()} />}
      </Match>
      <Match when={props.part.type === "tool" && (props.part as ToolPart)}>{(part) => <ToolCard part={part()} />}</Match>
      <Match when={props.part.type === "retry" && props.part}>
        {(part) => <div class="text-xs text-warn">retrying (attempt {(part() as { attempt: number }).attempt})</div>}
      </Match>
      <Match when={props.part.type === "compaction"}>
        <div class="my-2 flex items-center gap-3 text-xs text-ink-faint">
          <div class="h-px flex-1 bg-edge" />
          context compacted
          <div class="h-px flex-1 bg-edge" />
        </div>
      </Match>
      <Match when={props.part.type === "subtask" && props.part}>
        {(part) => (
          <div class="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-ink-muted">
            <span class="text-ink-faint">subtask · {(part() as { agent: string }).agent}</span>{" "}
            {(part() as { description: string }).description}
          </div>
        )}
      </Match>
      <Match when={props.part.type === "file" && props.part}>
        {(part) => (
          <span class="inline-flex items-center gap-1 rounded-md border border-edge bg-raised px-2 py-0.5 text-xs text-ink-muted">
            {(part() as { filename?: string }).filename ?? "attachment"}
          </span>
        )}
      </Match>
      <Match when={props.part.type === "patch" && props.part}>
        {(part) => (
          <div class="text-xs text-ink-faint">patched {(part() as { files: string[] }).files.length} file(s)</div>
        )}
      </Match>
    </Switch>
  )
}

function visibleText(part: Part) {
  if (part.type !== "text" || part.synthetic || part.ignored || !part.text.trim()) return undefined
  return part
}

function ReasoningView(props: { part: ReasoningPart }) {
  const [open, setOpen] = createSignal(false)
  const thinking = () => !props.part.time.end
  return (
    <div class="text-sm">
      <button
        class="flex items-center gap-1.5 text-ink-faint transition-colors hover:text-ink-muted"
        classList={{ "pulse-soft": thinking() }}
        onClick={() => setOpen(!open())}
      >
        <Chevron open={open()} />
        {thinking() ? "Thinking" : "Thought"}
      </button>
      <Show when={open()}>
        <div class="mt-1.5 border-l-2 border-edge pl-3 text-ink-muted">
          <Markdown text={props.part.text} done={!thinking()} />
        </div>
      </Show>
    </div>
  )
}

const statusColor: Record<string, string> = {
  pending: "bg-ink-faint",
  running: "bg-accent pulse-soft",
  completed: "bg-ok",
  error: "bg-danger",
}

function ToolCard(props: { part: ToolPart }) {
  const [open, setOpen] = createSignal(false)
  const state = () => props.part.state
  const title = () => {
    const s = state()
    return ("title" in s && s.title) || props.part.tool
  }
  return (
    <div class="overflow-hidden rounded-lg border border-edge bg-surface text-sm">
      <button
        class="flex w-full items-center gap-2.5 px-3 py-2 text-left transition-colors hover:bg-raised"
        onClick={() => setOpen(!open())}
      >
        <span class={`size-1.5 shrink-0 rounded-full ${statusColor[state().status]}`} />
        <span class="font-mono text-xs text-ink-faint">{props.part.tool}</span>
        <span class="min-w-0 flex-1 truncate text-ink-muted">{title()}</span>
        <Chevron open={open()} />
      </button>
      <Show when={open()}>
        <div class="space-y-2 border-t border-edge px-3 py-2">
          <ToolDetail label="input" value={JSON.stringify(state().input, null, 2)} />
          <Show when={state().status === "completed" && state()}>
            {(s) => <ToolDetail label="output" value={(s() as { output: string }).output} />}
          </Show>
          <Show when={state().status === "error" && state()}>
            {(s) => <ToolDetail label="error" value={(s() as { error: string }).error} danger />}
          </Show>
        </div>
      </Show>
    </div>
  )
}

function ToolDetail(props: { label: string; value: string; danger?: boolean }) {
  const clipped = () => {
    const clean = props.value.replace(/\u001b\[[0-9;]*m/g, "")
    return clean.length > 4000 ? clean.slice(0, 4000) + "\n..." : clean
  }
  return (
    <div>
      <div class="mb-0.5 text-[0.65rem] tracking-wide text-ink-faint uppercase">{props.label}</div>
      <pre
        class="max-h-64 overflow-auto rounded-md bg-raised p-2 font-mono text-xs whitespace-pre-wrap"
        classList={{ "text-danger": props.danger, "text-ink-muted": !props.danger }}
      >
        {clipped()}
      </pre>
    </div>
  )
}

export function Chevron(props: { open: boolean }) {
  return (
    <svg
      class="size-3 shrink-0 transition-transform duration-150"
      classList={{ "rotate-90": props.open }}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
    >
      <path d="M6 4l4 4-4 4" />
    </svg>
  )
}
