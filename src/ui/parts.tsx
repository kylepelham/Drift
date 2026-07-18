import type { Part, ReasoningPart, ToolPart } from "@opencode-ai/sdk/client"
import { createSignal, For, Match, Show, Switch } from "solid-js"
import { selectSession } from "../state/selection"
import { IconArrowUpRight } from "./icons"
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
  const spawnedId = () => {
    if (props.part.tool !== "task" && props.part.tool !== "spawn_thread") return null
    const s = state()
    const meta = ("metadata" in s ? s.metadata : undefined) ?? props.part.metadata
    return (meta as { sessionId?: string } | undefined)?.sessionId ?? null
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
        <Show when={spawnedId()}>
          {(childId) => (
            <span
              role="button"
              title="Open spawned thread"
              class="flex size-6 shrink-0 items-center justify-center rounded text-ink-faint transition-colors hover:bg-overlay hover:text-ink"
              onClick={(event) => {
                event.stopPropagation()
                selectSession(childId())
              }}
            >
              <IconArrowUpRight class="size-3.5" />
            </span>
          )}
        </Show>
        <Chevron open={open()} />
      </button>
      <Show when={open()}>
        <div class="border-t border-edge">
          <ToolBody part={props.part} />
        </div>
      </Show>
    </div>
  )
}

function ToolBody(props: { part: ToolPart }) {
  const state = () => props.part.state
  const meta = () => {
    const s = state()
    return (("metadata" in s ? s.metadata : undefined) ?? props.part.metadata) as Record<string, unknown> | undefined
  }
  const diff = () => (typeof meta()?.diff === "string" ? (meta()!.diff as string) : null)
  const shell = () => {
    if (props.part.tool !== "bash") return null
    const command = (state().input as { command?: string }).command
    const output =
      state().status === "completed"
        ? (state() as { output: string }).output
        : ((meta()?.output as string | undefined) ?? "")
    return { command: command ?? "", output }
  }
  return (
    <Switch
      fallback={
        <div class="space-y-2 px-3 py-2">
          <ToolDetail label="input" value={JSON.stringify(state().input, null, 2)} />
          <Show when={state().status === "completed" && state()}>
            {(s) => <ToolDetail label="output" value={(s() as { output: string }).output} />}
          </Show>
          <ErrorDetail state={state()} />
        </div>
      }
    >
      <Match when={diff()}>
        {(patch) => (
          <>
            <DiffView diff={patch()} />
            <ErrorDetail state={state()} padded />
          </>
        )}
      </Match>
      <Match when={shell()}>
        {(run) => (
          <div class="space-y-1.5 px-3 py-2">
            <pre class="overflow-x-auto rounded-md bg-raised p-2 font-mono text-xs whitespace-pre-wrap text-ink">
              <span class="text-accent select-none">$ </span>
              {run().command}
            </pre>
            <Show when={run().output.trim()}>
              <pre class="max-h-64 overflow-auto rounded-md bg-raised p-2 font-mono text-xs whitespace-pre-wrap text-ink-muted">
                {clip(stripAnsi(run().output))}
              </pre>
            </Show>
            <ErrorDetail state={state()} />
          </div>
        )}
      </Match>
    </Switch>
  )
}

function ErrorDetail(props: { state: ToolPart["state"]; padded?: boolean }) {
  return (
    <Show when={props.state.status === "error" && props.state}>
      {(s) => (
        <div classList={{ "px-3 pb-2": props.padded }}>
          <ToolDetail label="error" value={(s() as { error: string }).error} danger />
        </div>
      )}
    </Show>
  )
}

function DiffView(props: { diff: string }) {
  const lines = () =>
    props.diff
      .split("\n")
      .filter((line) => !line.startsWith("---") && !line.startsWith("+++") && !line.startsWith("\\") && line !== "")
  return (
    <div class="max-h-72 overflow-auto p-1 font-mono text-xs leading-relaxed">
      <For each={lines()}>
        {(line) => (
          <div
            class="rounded-xs px-2 whitespace-pre"
            classList={{
              "bg-ok/10 text-ok": line.startsWith("+"),
              "bg-danger/10 text-danger": line.startsWith("-"),
              "py-0.5 text-ink-faint": line.startsWith("@@"),
              "text-ink-muted": !line.startsWith("+") && !line.startsWith("-") && !line.startsWith("@@"),
            }}
          >
            {line}
          </div>
        )}
      </For>
    </div>
  )
}

function stripAnsi(value: string) {
  return value.replace(/\u001b\[[0-9;]*m/g, "")
}

function clip(value: string) {
  return value.length > 4000 ? value.slice(0, 4000) + "\n..." : value
}

function ToolDetail(props: { label: string; value: string; danger?: boolean }) {
  return (
    <div>
      <div class="mb-0.5 text-[0.65rem] tracking-wide text-ink-faint uppercase">{props.label}</div>
      <pre
        class="max-h-64 overflow-auto rounded-md bg-raised p-2 font-mono text-xs whitespace-pre-wrap"
        classList={{ "text-danger": props.danger, "text-ink-muted": !props.danger }}
      >
        {clip(stripAnsi(props.value))}
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
