import type { FilePart, Part, ReasoningPart, ToolPart } from "@opencode-ai/sdk/client"
import { createEffect, createMemo, createSignal, For, Match, Show, Switch, type JSX } from "solid-js"
import { useEngine } from "../engine"
import { hasPartRenderer, hasToolRenderer, PluginPartView, PluginToolView } from "../plugins"
import { openLightbox } from "./lightbox"
import { showReasoning } from "../state/prefs"
import { selectSession } from "../state/selection"
import { IconArrowUpRight, IconBranch, IconCheck, IconCopy } from "./icons"
import { codeTokens, Markdown, ProgressiveCodeView, type SyntaxToken } from "./markdown"
import { TextShimmer } from "./text-shimmer"
import { openToolContextMenu } from "./tool-context-menu"

export const contextTools = new Set(["read", "glob", "grep", "list"])
const hiddenTools = new Set(["todowrite", "todoread"])

export function PartView(props: { part: Part }) {
  return (
    <Switch>
      <Match when={props.part.type !== "tool" && hasPartRenderer(props.part.type) && props.part}>
        {(part) => <PluginPartView part={part()} />}
      </Match>
      <Match when={visibleText(props.part)}>{(part) => <Markdown text={part().text} done={!!part().time?.end} />}</Match>
      <Match when={showReasoning() && props.part.type === "reasoning" && (props.part as ReasoningPart)}>
        {(part) => <ReasoningView part={part()} />}
      </Match>
      <Match
        when={
          props.part.type === "tool" &&
          hasToolRenderer((props.part as ToolPart).tool) &&
          (props.part as ToolPart)
        }
      >
        {(part) => (
          <ToolContextTarget part={part()}>
            <PluginToolView part={part()} />
          </ToolContextTarget>
        )}
      </Match>
      <Match when={props.part.type === "tool" && !hiddenTools.has((props.part as ToolPart).tool) && (props.part as ToolPart)}>
        {(part) => (
          <ToolContextTarget part={part()}>
            <ToolView part={part()} />
          </ToolContextTarget>
        )}
      </Match>
      <Match when={props.part.type === "retry" && props.part}>
        {(part) => <div class="text-xs text-warn">retrying (attempt {(part() as { attempt: number }).attempt})</div>}
      </Match>
      <Match when={props.part.type === "compaction"}>
        <div class="my-2 flex items-center gap-3 text-xs text-ink-faint">
          <div class="h-px flex-1 bg-edge" />
          Context compacted
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
        {(part) => <FilePartView part={part() as FilePart} />}
      </Match>
    </Switch>
  )
}

function ToolContextTarget(props: { part: ToolPart; children: JSX.Element }) {
  return <div onContextMenu={(event) => openToolContextMenu(event, props.part)}>{props.children}</div>
}

function visibleText(part: Part) {
  if (part.type !== "text" || part.synthetic || part.ignored || !part.text.trim()) return undefined
  return part
}

export function partVisible(part: Part) {
  if (part.type !== "tool" && hasPartRenderer(part.type)) return true
  switch (part.type) {
    case "text":
      return !!visibleText(part)
    case "reasoning":
      return showReasoning()
    case "tool":
      return !hiddenTools.has((part as ToolPart).tool)
    case "retry":
    case "compaction":
    case "subtask":
    case "file":
      return true
    default:
      return false
  }
}

export function FilePartView(props: { part: Pick<FilePart, "mime" | "filename" | "url"> }) {
  const linkable = () => props.part.url.startsWith("data:") || props.part.url.startsWith("http")
  return (
    <Switch
      fallback={
        <Show
          when={linkable()}
          fallback={
            <span class="inline-flex max-w-full items-center gap-1.5 rounded-md border border-edge bg-raised px-2 py-1 text-xs text-ink-muted">
              <span class="truncate">{props.part.filename ?? "attachment"}</span>
            </span>
          }
        >
          <a
            href={props.part.url}
            download={props.part.filename ?? "attachment"}
            title="Download"
            class="inline-flex max-w-full items-center gap-1.5 rounded-md border border-edge bg-raised px-2 py-1 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink"
          >
            <span class="truncate">{props.part.filename ?? "attachment"}</span>
          </a>
        </Show>
      }
    >
      <Match when={props.part.mime.startsWith("image/") && linkable()}>
        <ImageThumb url={props.part.url} filename={props.part.filename} mime={props.part.mime} />
      </Match>
      <Match when={props.part.mime.startsWith("audio/") && linkable()}>
        <audio controls src={props.part.url} class="max-w-full" />
      </Match>
      <Match when={props.part.mime.startsWith("video/") && linkable()}>
        <video controls src={props.part.url} class="max-h-64 max-w-full rounded-lg border border-edge" />
      </Match>
    </Switch>
  )
}

function ImageThumb(props: { url: string; filename?: string; mime?: string }) {
  return (
    <button
      title={props.filename ?? "View image"}
      class="block overflow-hidden rounded-md border border-edge transition-colors hover:border-edge-strong"
      onClick={() => openLightbox({ url: props.url, filename: props.filename, mime: props.mime })}
    >
      <img src={props.url} alt={props.filename ?? ""} class="size-20 object-cover" />
    </button>
  )
}

function ReasoningView(props: { part: ReasoningPart }) {
  const [open, setOpen] = createSignal(false)
  const thinking = () => !props.part.time.end
  return (
    <div class="text-sm">
      <button
        class="flex items-center gap-1.5 text-ink-faint transition-colors hover:text-ink-muted"
        onClick={() => setOpen(!open())}
      >
        <Chevron open={open()} />
        <TextShimmer text={thinking() ? "Thinking" : "Thought"} active={thinking()} />
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
type PatchFile = {
  filePath: string
  relativePath?: string
  type?: "add" | "update" | "delete" | "move"
  patch: string
  additions: number
  deletions: number
}

function toolInfo(part: ToolPart): ToolInfo {
  const input = part.state.input as Record<string, unknown>
  const meta = toolMeta(part) ?? {}
  const text = (key: string) => (typeof input?.[key] === "string" ? (input[key] as string) : undefined)
  const output = () => (part.state.status === "completed" ? (part.state as { output: string }).output : "")
  const count = (value: unknown, singular: string, plural = `${singular}s`) =>
    typeof value === "number" ? ` · ${value} ${value === 1 ? singular : plural}${meta.truncated ? "+" : ""}` : ""
  switch (part.tool) {
    case "bash":
      return { title: "Shell", subtitle: text("command"), mono: true }
    case "edit":
      return { title: "Edit", subtitle: filename(text("filePath")) }
    case "write":
      return { title: "Write", subtitle: filename(text("filePath")) }
    case "apply_patch": {
      const files = patchFiles(part).length || (Array.isArray(input?.files) ? input.files.length : 0)
      return { title: "Patch", subtitle: files ? `${files} file${files > 1 ? "s" : ""}` : undefined }
    }
    case "read": {
      const lines = output() ? output().split("\n").length : undefined
      return { title: "Read", subtitle: `${filename(text("filePath")) ?? ""}${count(lines, "line")}` }
    }
    case "list":
      return { title: "List", subtitle: `${filename(text("path")) ?? ""}${count(meta.count, "entry", "entries")}` }
    case "glob":
      return { title: "Glob", subtitle: `${text("pattern") ?? ""}${count(meta.count, "file")}`, mono: true }
    case "grep":
      return { title: "Grep", subtitle: `${text("pattern") ?? ""}${count(meta.matches, "match", "matches")}`, mono: true }
    case "webfetch":
      return { title: "Fetch", subtitle: text("url"), mono: true }
    case "websearch": {
      const results = output() ? (output().match(/^#|^\d+\./gm)?.length ?? undefined) : undefined
      return { title: "Search", subtitle: `${text("query") ?? ""}${count(results, "result")}` }
    }
    case "task": {
      const agent = text("subagent_type")
      return { title: agent ? agent.charAt(0).toUpperCase() + agent.slice(1) : "Task", subtitle: text("description") }
    }
    case "spawn_thread":
      return { title: "Spawn", subtitle: text("title") }
    case "question":
      return { title: "Question", subtitle: text("question") ?? (input?.questions as { header?: string }[] | undefined)?.[0]?.header }
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

function extension(name?: string) {
  return name?.match(/\.(\w+)$/)?.[1]?.toLowerCase() ?? "text"
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

function awaitingPermission(
  state: {
    permissions: Record<string, { callID?: string }[]>
    questions: Record<string, { tool?: { callID: string } }[]>
  },
  part: ToolPart,
) {
  return (
    (state.permissions[part.sessionID] ?? []).some((permission) => permission.callID === part.callID) ||
    (state.questions[part.sessionID] ?? []).some((question) => question.tool?.callID === part.callID)
  )
}

function toolMeta(part: ToolPart) {
  const state = part.state
  return (("metadata" in state ? state.metadata : undefined) ?? part.metadata) as Record<string, unknown> | undefined
}

export function patchFiles(part: ToolPart) {
  const files = toolMeta(part)?.files
  if (!Array.isArray(files)) return []
  return files.filter(
    (file): file is PatchFile =>
      !!file &&
      typeof file === "object" &&
      typeof (file as PatchFile).filePath === "string" &&
      typeof (file as PatchFile).patch === "string" &&
      typeof (file as PatchFile).additions === "number" &&
      typeof (file as PatchFile).deletions === "number",
  )
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
  const engine = useEngine()
  const state = () => props.part.state
  const info = () => toolInfo(props.part)
  const delegated = () => props.part.tool === "task" || props.part.tool === "spawn_thread"
  const active = () =>
    !awaitingPermission(engine.state, props.part) && (state().status === "running" || state().status === "pending")
  const title = () => (info().called ? `Called ${info().called}` : (info().title ?? props.part.tool))
  const progress = () => {
    const childId = spawnedId()
    if (!childId || state().status !== "running") return null
    const activity = engine.state.activity[childId]
    if (!activity) return null
    return `${activity.tools} tool${activity.tools === 1 ? "" : "s"}${activity.current ? " · " + activity.current : ""}`
  }
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
    <div class="flex min-w-0 max-w-full flex-col gap-1 text-sm">
      <button
        class="flex min-h-8 w-full min-w-0 max-w-full items-center gap-2 overflow-hidden rounded-md px-1.5 text-left transition-colors hover:bg-raised/40"
        classList={{ "border-l-2 border-accent/35 pl-2": delegated() }}
        onClick={() => {
          const childId = spawnedId()
          if (childId && state().status !== "completed" && state().status !== "error") return selectSession(childId)
          setOpen(!open())
        }}
      >
        <Show when={error()}>
          <span class="size-3.5 shrink-0 text-danger">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="size-3.5">
              <circle cx="12" cy="12" r="9" />
              <path d="M5.5 5.5l13 13" />
            </svg>
          </span>
        </Show>
        <Show when={awaitingPermission(engine.state, props.part)}>
          <span class="size-1.5 shrink-0 rounded-full bg-warn" title="Waiting for permission" />
        </Show>
        <Show when={delegated()}>
          <IconBranch class="size-3.5 shrink-0 text-accent/70" />
          <span class="shrink-0 text-[0.62rem] font-semibold tracking-[0.12em] text-ink-faint uppercase">Delegate</span>
        </Show>
        <Show
          when={active()}
          fallback={
            <Show when={info().called} fallback={<span class="shrink-0 font-semibold text-ink">{info().title}</span>}>
              <span class="shrink-0 font-semibold text-ink">
                Called <code class="rounded bg-raised px-1 font-mono text-xs font-normal">{info().called}</code>
              </span>
            </Show>
          }
        >
          <TextShimmer text={title()} class="shrink-0 font-semibold" />
        </Show>
        <Show when={info().subtitle && !active() && !(props.part.tool === "bash" && expanded())}>
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
        <Show when={progress()}>
          {(text) => <span class="shrink-0 font-mono text-xs text-accent/80">{text()}</span>}
        </Show>
        <Show when={awaitingPermission(engine.state, props.part)}>
          <span class="shrink-0 text-xs text-warn/90">waiting for permission</span>
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
        <Show when={!active()}>
          <Chevron open={expanded()} />
        </Show>
      </button>
      <Show when={expanded()}>
        <div class="min-w-0 max-w-full">
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
  const patched = () => (props.part.tool === "apply_patch" ? patchFiles(props.part) : [])
  const diffLanguage = () => {
    const input = state().input as { filePath?: string }
    const file = patched()[0]
    return extension(file?.relativePath ?? file?.filePath ?? input.filePath)
  }
  const tasked = () => taskBody(props.part)
  return (
    <>
      <Switch fallback={<GenericBody part={props.part} />}>
        <Match when={tasked()}>
          {(task) => (
            <div class="space-y-2 border-l-2 border-edge pl-3">
              <Show when={task().prompt}>
                <div class="max-h-40 overflow-auto text-[0.85rem] whitespace-pre-wrap text-ink-faint">
                  {clip(task().prompt)}
                </div>
              </Show>
              <Show when={task().result}>
                <div class="max-h-80 overflow-auto text-ink-muted">
                  <Markdown text={task().result} done />
                </div>
              </Show>
            </div>
          )}
        </Match>
        <Match when={shell()}>
          {(run) => <ShellOutput command={run().command} output={run().output} />}
        </Match>
        <Match when={written()}>
          {(file) => (
            <ProgressiveCodeView code={file().content} lang={extension(file().name)} />
          )}
        </Match>
        <Match when={patched().length > 1 && patched()}>{(files) => <PatchPanel files={files()} />}</Match>
        <Match when={props.diff}>{(patch) => <DiffPanel diff={patch()} lang={diffLanguage()} />}</Match>
      </Switch>
      <Show when={props.error}>
        {(message) => (
          <div class="mt-1.5 border-l-2 border-danger py-0.5 pl-3 text-[0.85rem] break-words whitespace-pre-wrap text-ink-muted">
            {clip(stripAnsi(message()))}
          </div>
        )}
      </Show>
    </>
  )
}

export function shellTranscript(command: string, output: string) {
  const normalized = stripAnsi(output).replace(/\r\n?/g, "\n")
  return `$ ${command}${normalized.trim() ? `\n\n${normalized}` : ""}`
}

function ShellLine(props: { line: string; last: boolean }) {
  const command = () => (props.line.startsWith("$ ") ? props.line.slice(2) : null)
  return (
    <>
      <Show when={command() !== null} fallback={<span class="text-ink-muted">{props.line}</span>}>
        <span class="text-accent select-none">$ </span>
        <span class="font-medium text-ink">{command()}</span>
      </Show>
      {props.last ? "" : "\n"}
    </>
  )
}

function ShellOutput(props: { command: string; output: string }) {
  const [copied, setCopied] = createSignal(false)
  const transcript = () => shellTranscript(props.command, props.output)
  const lines = () => transcript().split("\n")
  const copy = async () => {
    await navigator.clipboard.writeText(transcript())
    setCopied(true)
    setTimeout(() => setCopied(false), 2000)
  }
  return (
    <div class="group/shell relative overflow-hidden rounded-[6px] border-[0.5px] border-edge">
      <button
        title="Copy shell output"
        class="absolute top-1 right-1 z-10 flex size-6 items-center justify-center rounded text-ink-faint opacity-0 transition-opacity group-focus-within/shell:opacity-100 group-hover/shell:opacity-100 hover:bg-raised hover:text-ink"
        onClick={() => void copy()}
      >
        <Show when={copied()} fallback={<IconCopy class="size-3.5" />}>
          <IconCheck class="size-3.5" />
        </Show>
      </button>
      <pre
        class="shell-output max-h-60 overflow-x-hidden overflow-y-auto p-3 pr-10 font-mono text-[13px] leading-[1.5] whitespace-pre-wrap text-ink"
        role="region"
        aria-label="Shell output"
        tabIndex={0}
      >
        <code class="break-words">
          <For each={lines()}>{(line, index) => <ShellLine line={line} last={index() === lines().length - 1} />}</For>
        </code>
      </pre>
    </div>
  )
}

export function taskBody(part: ToolPart) {
  if (part.tool !== "task" && part.tool !== "spawn_thread") return null
  const input = part.state.input as { prompt?: string; task?: string }
  const output = part.state.status === "completed" ? (part.state as { output: string }).output : ""
  const result = output.match(/<task_result>\n?([\s\S]*?)\n?<\/task_result>/)?.[1] ?? output
  return { prompt: input.prompt ?? input.task ?? "", result }
}

function GenericBody(props: { part: ToolPart }) {
  const state = () => props.part.state
  const output = () => (state().status === "completed" ? (state() as { output: string }).output : "")
  const showInput = () => !!toolInfo(props.part).called
  return (
    <div class="space-y-1.5 border-l-2 border-edge pl-3">
      <Show when={showInput()}>
        <div class="font-mono text-xs break-all whitespace-pre-wrap text-ink-faint">
          {JSON.stringify(state().input, null, 1)}
        </div>
      </Show>
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

function DiffPanel(props: { diff: string; lang: string; bare?: boolean }) {
  const rows = createMemo(() => parseDiff(props.diff))
  const [tokens, setTokens] = createSignal<SyntaxToken[][]>([])
  let request = 0
  createEffect(() => {
    const current = ++request
    const code = rows().map((row) => row.text).join("\n")
    void codeTokens(code, props.lang).then((result) => current === request && setTokens(result))
  })
  return (
    <div class="overflow-hidden" classList={{ "rounded-lg border border-edge": !props.bare }}>
      <div class="max-h-80 overflow-auto py-1 font-mono text-xs leading-relaxed">
        <For each={rows()}>
          {(row, index) => (
            <Show when={row.kind !== "gap"} fallback={<div class="my-1 border-t border-dashed border-edge" />}>
              <div
                class="flex"
                classList={{ "bg-ok/10": row.kind === "add", "bg-danger/10": row.kind === "del" }}
              >
                <span class="w-10 shrink-0 pr-2 text-right text-ink-faint select-none">{row.line}</span>
                <span class="min-w-0 flex-1 pr-3 whitespace-pre-wrap text-ink-muted">
                  <Show
                    when={tokens()[index()]?.length ? tokens()[index()] : undefined}
                    fallback={
                      <span classList={{ "text-ok": row.kind === "add", "text-danger": row.kind === "del" }}>
                        {row.text}
                      </span>
                    }
                  >
                    {(line) => <For each={line()}>{(token) => <SyntaxTokenView token={token} />}</For>}
                  </Show>
                </span>
              </div>
            </Show>
          )}
        </For>
      </div>
    </div>
  )
}

function SyntaxTokenView(props: { token: SyntaxToken }) {
  const style = () => {
    const fontStyle = props.token.fontStyle ?? 0
    return {
      color: props.token.color,
      "font-style": fontStyle & 1 ? "italic" : undefined,
      "font-weight": fontStyle & 2 ? "bold" : undefined,
      "text-decoration": fontStyle & 4 ? "underline" : undefined,
    }
  }
  return <span style={style()}>{props.token.content}</span>
}

function PatchPanel(props: { files: PatchFile[] }) {
  return (
    <div class="space-y-2">
      <For each={props.files}>{(file) => <PatchFilePanel file={file} />}</For>
    </div>
  )
}

function PatchFilePanel(props: { file: PatchFile }) {
  const [open, setOpen] = createSignal(true)
  const status = () => {
    if (props.file.type === "add") return "Created"
    if (props.file.type === "delete") return "Deleted"
    if (props.file.type === "move") return "Moved"
    return null
  }
  return (
    <div class="overflow-hidden rounded-lg border border-edge">
      <button
        class="flex w-full items-center gap-3 px-3 py-2 text-left transition-colors hover:bg-raised/50"
        onClick={() => setOpen(!open())}
      >
        <span class="min-w-0 flex-1 truncate font-mono text-xs text-ink-muted">
          {props.file.relativePath ?? props.file.filePath}
        </span>
        <span class="shrink-0 font-mono text-xs">
          <span class="text-ok">+{props.file.additions}</span> <span class="text-danger">-{props.file.deletions}</span>
        </span>
        <Show when={status()}>{(label) => <span class="shrink-0 text-xs text-ink-faint">{label()}</span>}</Show>
        <Chevron open={open()} />
      </button>
      <Show when={open()}>
        <div class="border-t border-edge">
          <DiffPanel diff={props.file.patch} lang={extension(props.file.relativePath ?? props.file.filePath)} bare />
        </div>
      </Show>
    </div>
  )
}

export function ExploredGroup(props: { parts: ToolPart[] }) {
  const engine = useEngine()
  const [open, setOpen] = createSignal(false)
  const label = () => `${props.parts.length} ${props.parts.length === 1 ? "read" : "reads"}`
  const waiting = () => props.parts.some((part) => awaitingPermission(engine.state, part))
  const running = () =>
    props.parts.some((part) => part.state.status === "running" || part.state.status === "pending")
  const expanded = () => open() || waiting()
  return (
    <div class="flex flex-col gap-1.5 text-sm">
      <button
        class="flex min-h-8 items-center gap-2 rounded-md px-1.5 text-left transition-colors hover:bg-raised/40"
        onClick={() => setOpen(!open())}
      >
        <Show when={waiting()}>
          <span class="size-1.5 shrink-0 rounded-full bg-warn" title="Waiting for permission" />
        </Show>
        <Show when={!waiting() && running()} fallback={<span class="font-semibold text-ink">Explored</span>}>
          <TextShimmer text="Explored" class="font-semibold" />
        </Show>
        <span class="text-ink-faint">{label()}</span>
        <Show when={waiting()}>
          <span class="text-xs text-warn/90">waiting for permission</span>
        </Show>
        <Chevron open={expanded()} />
      </button>
      <Show when={expanded()}>
        <div class="ml-2 flex flex-col gap-0.5 border-l-2 border-edge pl-3">
          <For each={props.parts}>{(part) => <ToolView part={part} />}</For>
        </div>
      </Show>
    </div>
  )
}

function stripAnsi(value: string) {
  return value.replace(/\u001b(?:\[[0-?]*[ -/]*[@-~]|[@-_])/g, "")
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
