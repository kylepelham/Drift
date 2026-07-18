import type { Part, ReasoningPart, ToolPart } from "@opencode-ai/sdk/client"
import { createSignal, For, Match, Show, Switch } from "solid-js"
import { selectSession } from "../state/selection"
import { IconArrowUpRight } from "./icons"
import { Markdown } from "./markdown"

export const contextTools = new Set(["read", "glob", "grep", "list"])
const hiddenTools = new Set(["todowrite", "todoread"])

export function PartView(props: { part: Part }) {
  return (
    <Switch>
      <Match when={visibleText(props.part)}>{(part) => <Markdown text={part().text} done={!!part().time?.end} />}</Match>
      <Match when={props.part.type === "reasoning" && (props.part as ReasoningPart)}>
        {(part) => <ReasoningView part={part()} />}
      </Match>
      <Match when={props.part.type === "tool" && !hiddenTools.has((props.part as ToolPart).tool) && (props.part as ToolPart)}>
        {(part) => <ToolView part={part()} />}
      </Match>
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
          <div class="text-sm text-ink-muted">
            <span class="font-semibold text-ink">Subtask</span>{" "}
            <span class="text-ink-faint">{(part() as { agent: string }).agent}</span>{" "}
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

type ToolInfo = { title?: string; called?: string; subtitle?: string; mono?: boolean }

function toolInfo(part: ToolPart): ToolInfo {
  const input = part.state.input as Record<string, unknown>
  const text = (key: string) => (typeof input?.[key] === "string" ? (input[key] as string) : undefined)
  switch (part.tool) {
    case "bash":
      return { title: "Shell", subtitle: text("command"), mono: true }
    case "edit":
      return { title: "Edit", subtitle: filename(text("filePath")) }
    case "write":
      return { title: "Write", subtitle: filename(text("filePath")) }
    case "apply_patch": {
      const files = Array.isArray(input?.files) ? input.files.length : 0
      return { title: "Patch", subtitle: files ? `${files} file${files > 1 ? "s" : ""}` : undefined }
    }
    case "read":
      return { title: "Read", subtitle: filename(text("filePath")) }
    case "list":
      return { title: "List", subtitle: filename(text("path")) }
    case "glob":
      return { title: "Glob", subtitle: text("pattern"), mono: true }
    case "grep":
      return { title: "Grep", subtitle: text("pattern"), mono: true }
    case "webfetch":
      return { title: "Fetch", subtitle: text("url"), mono: true }
    case "websearch":
      return { title: "Search", subtitle: text("query") }
    case "task": {
      const agent = text("subagent_type")
      return { title: agent ? agent.charAt(0).toUpperCase() + agent.slice(1) : "Task", subtitle: text("description") }
    }
    case "spawn_thread":
      return { title: "Spawn", subtitle: text("title") }
    case "question":
      return { title: "Question" }
    case "skill":
      return { title: text("name") ?? "Skill" }
    default:
      return { called: part.tool, subtitle: argsPreview(input), mono: true }
  }
}

function filename(path?: string) {
  if (!path) return undefined
  return path.replaceAll("\\", "/").split("/").filter(Boolean).at(-1)
}

function argsPreview(input: Record<string, unknown> | undefined) {
  if (!input) return undefined
  const parts = Object.entries(input).map(([key, value]) => {
    const raw = typeof value === "string" ? value : JSON.stringify(value)
    return `${key}=${raw}`
  })
  const joined = parts.join("  ")
  return joined.length > 120 ? joined.slice(0, 120) + "..." : joined
}

function toolMeta(part: ToolPart) {
  const state = part.state
  return (("metadata" in state ? state.metadata : undefined) ?? part.metadata) as Record<string, unknown> | undefined
}

function diffStats(diff: string) {
  let add = 0
  let del = 0
  for (const line of diff.split("\n")) {
    if (line.startsWith("+") && !line.startsWith("+++")) add++
    if (line.startsWith("-") && !line.startsWith("---")) del++
  }
  return { add, del }
}

export function ToolView(props: { part: ToolPart }) {
  const state = () => props.part.state
  const info = () => toolInfo(props.part)
  const diff = () => {
    const value = toolMeta(props.part)?.diff
    return typeof value === "string" && value.trim() ? value : null
  }
  const error = () => (state().status === "error" ? (state() as { error: string }).error : null)
  const [open, setOpen] = createSignal(props.part.tool === "bash")
  const expanded = () => open() || !!error()
  const stats = () => {
    const patch = diff()
    return patch ? diffStats(patch) : null
  }
  const spawnedId = () => {
    if (props.part.tool !== "task" && props.part.tool !== "spawn_thread") return null
    return (toolMeta(props.part) as { sessionId?: string } | undefined)?.sessionId ?? null
  }
  return (
    <div class="text-sm">
      <button
        class="-mx-1.5 flex max-w-full items-center gap-2 rounded-md px-1.5 py-0.5 text-left transition-colors hover:bg-raised/60"
        onClick={() => setOpen(!open())}
      >
        <Show when={error()}>
          <span class="size-3.5 shrink-0 text-danger">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="size-3.5">
              <circle cx="12" cy="12" r="9" />
              <path d="M5.5 5.5l13 13" />
            </svg>
          </span>
        </Show>
        <Show when={state().status === "running" || state().status === "pending"}>
          <span class="pulse-soft size-1.5 shrink-0 rounded-full bg-accent" />
        </Show>
        <Show when={info().called} fallback={<span class="shrink-0 font-semibold text-ink">{info().title}</span>}>
          <span class="shrink-0 font-semibold text-ink">
            Called <code class="rounded bg-raised px-1 font-mono text-xs font-normal">{info().called}</code>
          </span>
        </Show>
        <Show when={info().subtitle}>
          <span
            class="min-w-0 truncate text-ink-faint"
            classList={{ "font-mono text-xs": info().mono, "text-[0.85rem]": !info().mono }}
          >
            {info().subtitle}
          </span>
        </Show>
        <Show when={stats()}>
          {(s) => (
            <span class="shrink-0 font-mono text-xs">
              <span class="text-ok">+{s().add}</span> <span class="text-danger">-{s().del}</span>
            </span>
          )}
        </Show>
        <Show when={spawnedId()}>
          {(childId) => (
            <span
              role="button"
              title="Open spawned thread"
              class="flex size-5 shrink-0 items-center justify-center rounded text-ink-faint transition-colors hover:bg-overlay hover:text-ink"
              onClick={(event) => {
                event.stopPropagation()
                selectSession(childId())
              }}
            >
              <IconArrowUpRight class="size-3.5" />
            </span>
          )}
        </Show>
        <Chevron open={expanded()} />
      </button>
      <Show when={expanded()}>
        <div class="mt-1.5 mb-1">
          <ToolBody part={props.part} diff={diff()} error={error()} />
        </div>
      </Show>
    </div>
  )
}

function ToolBody(props: { part: ToolPart; diff: string | null; error: string | null }) {
  const state = () => props.part.state
  const shell = () => {
    if (props.part.tool !== "bash") return null
    const command = (state().input as { command?: string }).command ?? ""
    const output =
      state().status === "completed"
        ? (state() as { output: string }).output
        : ((toolMeta(props.part)?.output as string | undefined) ?? "")
    return { command, output }
  }
  const written = () => {
    if (props.part.tool !== "write") return null
    const input = state().input as { content?: string; filePath?: string }
    return typeof input.content === "string" ? { content: input.content, name: filename(input.filePath) } : null
  }
  return (
    <>
      <Switch fallback={<GenericBody part={props.part} />}>
        <Match when={shell()}>
          {(run) => (
            <div class="rounded-lg border border-edge bg-surface px-3 py-2.5 font-mono text-xs leading-relaxed">
              <div class="whitespace-pre-wrap text-ink">
                <span class="text-accent select-none">$ </span>
                {run().command}
              </div>
              <Show when={run().output.trim()}>
                <div class="mt-2 max-h-64 overflow-auto whitespace-pre-wrap text-ink-muted">
                  {clip(stripAnsi(run().output))}
                </div>
              </Show>
            </div>
          )}
        </Match>
        <Match when={written()}>
          {(file) => (
            <pre class="max-h-80 overflow-auto rounded-lg border border-edge px-3 py-2 font-mono text-xs leading-relaxed whitespace-pre-wrap text-ink-muted">
              {clip(file().content)}
            </pre>
          )}
        </Match>
        <Match when={props.diff}>{(patch) => <DiffPanel diff={patch()} />}</Match>
      </Switch>
      <Show when={props.error}>
        {(message) => (
          <div class="mt-1.5 border-l-2 border-danger py-0.5 pl-3 text-[0.85rem] whitespace-pre-wrap text-ink-muted">
            {clip(stripAnsi(message()))}
          </div>
        )}
      </Show>
    </>
  )
}

function GenericBody(props: { part: ToolPart }) {
  const state = () => props.part.state
  const output = () => (state().status === "completed" ? (state() as { output: string }).output : "")
  return (
    <div class="space-y-1.5 border-l-2 border-edge pl-3">
      <div class="font-mono text-xs break-all whitespace-pre-wrap text-ink-faint">
        {JSON.stringify(state().input, null, 1)}
      </div>
      <Show when={output().trim()}>
        <div class="max-h-64 overflow-auto text-[0.85rem] whitespace-pre-wrap text-ink-muted">
          {clip(stripAnsi(output()))}
        </div>
      </Show>
    </div>
  )
}

type DiffRow = { kind: "add" | "del" | "ctx" | "gap"; line?: number; text: string }

function parseDiff(diff: string): DiffRow[] {
  const rows: DiffRow[] = []
  let oldLine = 0
  let newLine = 0
  for (const line of diff.split("\n")) {
    if (line.startsWith("---") || line.startsWith("+++") || line.startsWith("\\") || line.startsWith("Index:")) continue
    if (line.startsWith("===")) continue
    const hunk = line.match(/^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/)
    if (hunk) {
      oldLine = Number(hunk[1])
      newLine = Number(hunk[2])
      if (rows.length) rows.push({ kind: "gap", text: "" })
      continue
    }
    if (line.startsWith("+")) rows.push({ kind: "add", line: newLine++, text: line.slice(1) })
    else if (line.startsWith("-")) rows.push({ kind: "del", line: oldLine++, text: line.slice(1) })
    else {
      rows.push({ kind: "ctx", line: newLine, text: line.slice(1) })
      oldLine++
      newLine++
    }
  }
  return rows
}

function DiffPanel(props: { diff: string }) {
  const rows = () => parseDiff(props.diff)
  return (
    <div class="overflow-hidden rounded-lg border border-edge">
      <div class="max-h-80 overflow-auto py-1 font-mono text-xs leading-relaxed">
        <For each={rows()}>
          {(row) => (
            <Show when={row.kind !== "gap"} fallback={<div class="my-1 border-t border-dashed border-edge" />}>
              <div
                class="flex"
                classList={{ "bg-ok/10": row.kind === "add", "bg-danger/10": row.kind === "del" }}
              >
                <span class="w-10 shrink-0 pr-2 text-right text-ink-faint select-none">{row.line}</span>
                <span
                  class="min-w-0 flex-1 pr-3 whitespace-pre-wrap"
                  classList={{
                    "text-ok": row.kind === "add",
                    "text-danger": row.kind === "del",
                    "text-ink-muted": row.kind === "ctx",
                  }}
                >
                  {row.text}
                </span>
              </div>
            </Show>
          )}
        </For>
      </div>
    </div>
  )
}

export function ExploredGroup(props: { parts: ToolPart[] }) {
  const [open, setOpen] = createSignal(false)
  const label = () => `${props.parts.length} ${props.parts.length === 1 ? "read" : "reads"}`
  return (
    <div class="text-sm">
      <button
        class="-mx-1.5 flex items-center gap-2 rounded-md px-1.5 py-0.5 text-left transition-colors hover:bg-raised/60"
        onClick={() => setOpen(!open())}
      >
        <span class="font-semibold text-ink">Explored</span>
        <span class="text-ink-faint">{label()}</span>
        <Chevron open={open()} />
      </button>
      <Show when={open()}>
        <div class="mt-1 ml-2 space-y-0.5 border-l-2 border-edge pl-3">
          <For each={props.parts}>{(part) => <ToolView part={part} />}</For>
        </div>
      </Show>
    </div>
  )
}

function stripAnsi(value: string) {
  return value.replace(/\u001b\[[0-9;]*m/g, "")
}

function clip(value: string) {
  return value.length > 4000 ? value.slice(0, 4000) + "\n..." : value
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
