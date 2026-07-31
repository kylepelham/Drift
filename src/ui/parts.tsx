import type { FilePart, Part, ReasoningPart, ToolPart } from "@opencode-ai/sdk/client"
import { createEffect, createMemo, createSignal, For, Match, on, onCleanup, onMount, Show, Switch, type JSX } from "solid-js"
import { useEngine } from "../engine"
import { hasPartRenderer, hasToolRenderer, PluginPartView, PluginToolView } from "../plugins"
import { Chevron } from "./controls"
import { openLightbox } from "./lightbox"
import { showReasoning, toolErrorsExpanded } from "../state/prefs"
import { agentLabel, t } from "../state/i18n"
import { selectSession } from "../state/selection"
import { IconArrowUpRight, IconBranch, IconCheck, IconCopy } from "./icons"
import { codeTokens, Markdown, ProgressiveCodeView, type SyntaxToken } from "./markdown"
import { diffIndicator, diffLineNumbers, diffWordWrap, syntaxTheme } from "../state/code"
import { TextShimmer } from "./text-shimmer"
import { openToolContextMenu } from "./tool-context-menu"
import { permissionRequiresAttention } from "../state/permission-attention"
import type { EngineState } from "../engine/store"

export const contextTools = new Set(["read", "glob", "grep", "list"])
const hiddenTools = new Set(["todowrite", "todoread"])

// How long the shell copy button shows its "copied" state.
// NOTE: markdown.tsx uses 1600ms for its visually identical code-block copy button.
const copiedFeedbackMs = 2000
// Tool output beyond this is clipped before rendering; long outputs otherwise stall the view.
const maxInlineOutputChars = 4000
// Tool argument previews are collapsed to a single line of at most this length.
const maxArgsPreviewChars = 120
// When re-syncing streamed shell output, compare this many trailing characters of the previous
// chunk against the new one to confirm the stream is an append rather than a fresh transcript.
const overlapProbeChars = 64
// Scroll positions within this many pixels of the bottom count as "at the bottom".
const bottomSlopPx = 2
// Shiki packs font styling into a bitmask on each token; these are its FontStyle enum values.
const fontStyleItalic = 1
const fontStyleBold = 2
const fontStyleUnderline = 4

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
      <Match when={props.part.type === "compaction"}>
        <div class="my-2 flex items-center gap-3 text-xs text-ink-faint">
          <div class="h-px flex-1 bg-edge" />
          {t("drift.context.compacted")}
          <div class="h-px flex-1 bg-edge" />
        </div>
      </Match>
      <Match when={props.part.type === "subtask" && props.part}>
        {(part) => (
          <div class="text-sm text-ink-muted">
            <span class="font-semibold text-ink">{t("drift.tool.subtask")}</span>{" "}
            <span class="text-ink-faint">{agentLabel((part() as { agent: string }).agent)}</span>{" "}
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
              <span class="truncate">{props.part.filename ?? t("common.attachment")}</span>
            </span>
          }
        >
          <a
            href={props.part.url}
            download={props.part.filename ?? "attachment"}
            title={t("drift.attachment.download")}
            class="inline-flex max-w-full items-center gap-1.5 rounded-md border border-edge bg-raised px-2 py-1 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink"
          >
            <span class="truncate">{props.part.filename ?? t("common.attachment")}</span>
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
      title={props.filename ?? t("drift.attachment.viewImage")}
      class="block overflow-hidden rounded-md border border-edge transition-colors hover:border-edge-strong"
      onClick={() => openLightbox({ url: props.url, filename: props.filename, mime: props.mime })}
    >
      <img src={props.url} alt={props.filename ?? t("drift.attachment.image")} class="size-20 object-cover" />
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
        <TextShimmer text={thinking() ? t("drift.reasoning.thinking") : t("drift.reasoning.thought")} active={thinking()} />
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
  const count = (value: unknown, singular: string, plural: string) =>
    typeof value === "number"
      ? ` · ${t(value === 1 ? singular : plural, { count: `${value}${meta.truncated ? "+" : ""}` })}`
      : ""
  switch (part.tool) {
    case "bash":
      return { title: t("prompt.mode.shell"), subtitle: text("command"), mono: true }
    case "edit":
      return { title: t("settings.permissions.tool.edit.title"), subtitle: filename(text("filePath")) }
    case "write":
      return { title: t("settings.permissions.tool.edit.title"), subtitle: filename(text("filePath")) }
    case "apply_patch":
      return { title: t("settings.permissions.tool.edit.title"), subtitle: patchSubtitle(part) }
    case "read": {
      const lines = output() ? output().split("\n").length : undefined
      return {
        title: t("settings.permissions.tool.read.title"),
        subtitle: `${filename(text("filePath")) ?? ""}${count(lines, "drift.count.line.one", "drift.count.line.other")}`,
      }
    }
    case "list":
      return {
        title: t("settings.permissions.tool.list.title"),
        subtitle: `${filename(text("path")) ?? ""}${count(meta.count, "drift.count.entry.one", "drift.count.entry.other")}`,
      }
    case "glob":
      return {
        title: t("settings.permissions.tool.glob.title"),
        subtitle: `${text("pattern") ?? ""}${count(meta.count, "drift.count.file.one", "drift.count.file.other")}`,
        mono: true,
      }
    case "grep":
      return {
        title: t("settings.permissions.tool.grep.title"),
        subtitle: `${text("pattern") ?? ""}${count(meta.matches, "drift.count.match.one", "drift.count.match.other")}`,
        mono: true,
      }
    case "webfetch":
      return { title: t("drift.tool.fetch"), subtitle: text("url"), mono: true }
    case "websearch": {
      const results = output() ? (output().match(/^#|^\d+\./gm)?.length ?? undefined) : undefined
      return {
        title: t("common.search.placeholder"),
        subtitle: `${text("query") ?? ""}${count(results, "drift.count.result.one", "drift.count.result.other")}`,
      }
    }
    case "task": {
      const agent = text("subagent_type")
      return { title: taskHeading(agent, text("description")) }
    }
    case "spawn_thread": {
      const title = text("title")
      return { title: title ? `${t("drift.tool.spawn")} ${title}` : t("drift.tool.spawn") }
    }
    case "question":
      return { title: t("notification.question.title"), subtitle: text("question") ?? (input?.questions as { header?: string }[] | undefined)?.[0]?.header }
    case "skill":
      return { title: text("name") ?? t("prompt.slash.badge.skill") }
    default:
      return { called: part.tool, subtitle: argsPreview(input), mono: true }
  }
}

export function taskHeading(agent?: string, description?: string) {
  const label = agent ? agentLabel(agent) : t("drift.tool.task")
  return description ? `${label} ${description}` : label
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
  return joined.length > maxArgsPreviewChars ? joined.slice(0, maxArgsPreviewChars) + "..." : joined
}

function awaitingPermission(state: EngineState, part: ToolPart) {
  return (
    (state.permissions[part.sessionID] ?? []).some(
      (permission) => permission.callID === part.callID && permissionRequiresAttention(permission, state),
    ) ||
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

export function patchSubtitle(part: ToolPart) {
  const files = patchFiles(part)
  if (files.length === 1) return filename(files[0].relativePath ?? files[0].filePath)
  const input = part.state.input as { files?: unknown[] } | undefined
  const paths = patchInputPaths(part)
  if (!files.length && paths.length === 1) return filename(paths[0])
  const count = files.length || paths.length || input?.files?.length || 0
  return count ? t(count === 1 ? "drift.count.file.one" : "drift.count.file.other", { count }) : undefined
}

export function patchInputPaths(part: ToolPart) {
  const patchText = (part.state.input as { patchText?: unknown } | undefined)?.patchText
  if (typeof patchText !== "string") return []
  return [...patchText.matchAll(/^\*\*\* (?:Add|Update|Delete) File: (.+)$/gm)].map((match) => match[1].trim())
}

export function nextToolOpen(current: boolean, hadError: boolean, hasError: boolean, errorsExpanded: boolean) {
  return hasError && !hadError ? errorsExpanded : current
}

export function initialToolOpen(tool: string, status: ToolPart["state"]["status"], errorsExpanded: boolean) {
  if (status === "error") return errorsExpanded
  return tool === "bash"
}

const explicitToolOpen = new Map<string, boolean>()
const maxExplicitToolOpen = 1000

export function initialToolOpenForPart(
  partId: string,
  tool: string,
  status: ToolPart["state"]["status"],
  errorsExpanded: boolean,
) {
  return explicitToolOpen.get(partId) ?? initialToolOpen(tool, status, errorsExpanded)
}

export function rememberToolOpen(partId: string, open: boolean) {
  if (explicitToolOpen.get(partId) === open) return
  explicitToolOpen.delete(partId)
  explicitToolOpen.set(partId, open)
  if (explicitToolOpen.size > maxExplicitToolOpen) explicitToolOpen.delete(explicitToolOpen.keys().next().value!)
}

export function activateToolHeader(toggle: () => void) {
  toggle()
}

export function openSpawnedThread(event: Pick<MouseEvent, "stopPropagation">, childId: string, select: (id: string) => void) {
  event.stopPropagation()
  select(childId)
}

export function toolChevronVisible(active: boolean, delegated: boolean) {
  return !active || delegated
}

function diffStats(diff: string) {
  let additions = 0
  let deletions = 0
  for (const row of parseDiff(diff)) {
    if (row.kind === "add") additions++
    if (row.kind === "del") deletions++
  }
  return { additions, deletions }
}

export function ToolView(props: { part: ToolPart }) {
  const engine = useEngine()
  const state = () => props.part.state
  const info = () => toolInfo(props.part)
  const delegated = () => props.part.tool === "task" || props.part.tool === "spawn_thread"
  const active = () =>
    !awaitingPermission(engine.state, props.part) && (state().status === "running" || state().status === "pending")
  const title = () => (info().called ? `${t("drift.tool.called")} ${info().called}` : (info().title ?? props.part.tool))
  const progress = () => {
    const childId = spawnedId()
    if (!childId || state().status !== "running") return null
    const activity = engine.state.activity[childId]
    if (!activity) return null
    const count = t(activity.tools === 1 ? "drift.count.tool.one" : "drift.count.tool.other", { count: activity.tools })
    return `${count}${activity.current ? " · " + activity.current : ""}`
  }
  const diff = () => {
    const value = toolMeta(props.part)?.diff
    return typeof value === "string" && value.trim() ? value : null
  }
  const error = () => (state().status === "error" ? (state() as { error: string }).error : null)
  const [open, setOpen] = createSignal(
    initialToolOpenForPart(props.part.id, props.part.tool, state().status, toolErrorsExpanded()),
  )
  createEffect(on(error, (value, previous) => setOpen(nextToolOpen(open(), !!previous, !!value, toolErrorsExpanded()))))
  const expanded = () => open()
  const toggleOpen = () => {
    const next = !open()
    rememberToolOpen(props.part.id, next)
    setOpen(next)
  }
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
        classList={{ "delegate-tool border-accent/35": delegated() }}
        onClick={() => activateToolHeader(toggleOpen)}
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
          <span class="size-1.5 shrink-0 rounded-full bg-warn" title={t("drift.status.waitingForPermission")} />
        </Show>
        <Show when={delegated()}>
          <IconBranch class="delegate-tool-icon size-3.5 shrink-0 text-accent/70" />
        </Show>
        <Show
          when={active()}
          fallback={
            <Show when={info().called} fallback={<span class="shrink-0 font-semibold text-ink">{info().title}</span>}>
              <span class="shrink-0 font-semibold text-ink">
                {t("drift.tool.called")}{" "}
                <code class="rounded bg-raised px-1 font-mono text-xs font-normal">{info().called}</code>
              </span>
            </Show>
          }
        >
          <TextShimmer text={title()} class="shrink-0 font-semibold" />
        </Show>
        <Show when={info().subtitle && !(props.part.tool === "bash" && expanded())}>
          <span
            class="min-w-0 truncate text-ink-faint"
            classList={{ "font-mono text-xs": info().mono, "text-[0.85rem]": !info().mono }}
          >
            {info().subtitle}
          </span>
        </Show>
        <Show when={stats()}>
          {(counts) => (
            <span class="shrink-0 font-mono text-xs">
              <span class="text-ok">+{counts().additions}</span>{" "}
              <span class="text-danger">-{counts().deletions}</span>
            </span>
          )}
        </Show>
        <Show when={progress()}>
          {(text) => <span class="shrink-0 font-mono text-xs text-accent/80">{text()}</span>}
        </Show>
        <Show when={awaitingPermission(engine.state, props.part)}>
          <span class="shrink-0 text-xs text-warn/90">{t("drift.status.waitingForPermission")}</span>
        </Show>
        <Show when={spawnedId()}>
          {(childId) => (
            <span
              role="button"
              title={t("drift.thread.openSpawned")}
              class="flex size-5 shrink-0 items-center justify-center rounded text-ink-faint transition-colors hover:bg-overlay hover:text-ink"
              onClick={(event) => openSpawnedThread(event, childId(), selectSession)}
            >
              <IconArrowUpRight class="size-3.5" />
            </span>
          )}
        </Show>
        <Show when={toolChevronVisible(active(), delegated())}>
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
  const shellCommand = () => (state().input as { command?: string }).command ?? ""
  const shellOutput = () =>
    state().status === "completed"
      ? (state() as { output: string }).output
      : ((toolMeta(props.part)?.output as string | undefined) ?? "")
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
        <Match when={props.part.tool === "bash"}>
          <ShellOutput
            command={shellCommand()}
            output={shellOutput()}
            running={state().status === "pending" || state().status === "running"}
          />
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

/**
 * Renders streaming shell output incrementally.
 *
 * The engine re-sends the whole output on every update. Re-rendering all of it each time is too
 * slow for a long-running command, so this tracks how much has already been shown and emits only
 * the new tail when it can prove the new output is an append of the old one. When it cannot, it
 * emits a full replacement.
 *
 * Callers get `{ replace, text }`: `replace: true` means "this is the whole transcript",
 * `replace: false` means "append this".
 */
export function createShellTranscriptStream() {
  let command = ""
  /** How many characters of the raw output have already been turned into visible text. */
  let outputLength = 0
  let previousOutput = ""
  let initialized = false
  /** Set once non-whitespace output has been shown; until then output is buffered in `pending`. */
  let visible = false
  let finished = false
  /** ANSI escape parser state. CSI sequences run until a byte in the 0x40-0x7e final range. */
  let escape: "none" | "start" | "csi" = "none"
  /** A trailing CR is held over: it only becomes a newline once we know what follows it. */
  let carriageReturn = false
  /** Whitespace-only chunks seen before the first visible output, kept so none are lost. */
  let pending: string[] = []

  /** Strips ANSI escapes and folds CR / CRLF into newlines. `flush` resolves a trailing CR. */
  const consume = (value: string, flush: boolean) => {
    let normalized = ""
    for (const character of value) {
      if (escape === "start") {
        escape = character === "[" ? "csi" : "none"
        continue
      }
      if (escape === "csi") {
        const code = character.charCodeAt(0)
        if (code >= 0x40 && code <= 0x7e) escape = "none"
        continue
      }
      if (character === "\u001b") {
        escape = "start"
        continue
      }
      if (carriageReturn) {
        normalized += "\n"
        carriageReturn = false
        if (character === "\n") continue
      }
      if (character === "\r") carriageReturn = true
      else normalized += character
    }
    if (flush && carriageReturn) {
      normalized += "\n"
      carriageReturn = false
    }
    return normalized
  }

  /** Re-renders the whole transcript from scratch and re-primes the incremental state. */
  const reset = (nextCommand: string, output: string, done: boolean) => {
    command = nextCommand
    outputLength = output.length
    previousOutput = output
    initialized = true
    finished = done
    visible = false
    escape = "none"
    carriageReturn = false
    pending = []
    const normalized = consume(output, done)
    visible = !!normalized.trim()
    if (!visible) pending = [normalized]
    return { replace: true, text: `$ ${command}${visible ? `\n\n${normalized}` : ""}` }
  }

  return {
    update(nextCommand: string, output: string, done: boolean) {
      // Nothing to append against: first update, a different command, output that shrank, or a
      // final frame whose trailing CR still has to be flushed.
      if (!initialized || command !== nextCommand || output.length < outputLength || finished || done) {
        return reset(nextCommand, output, done)
      }
      // Same length: either a genuine no-op, or the content changed underneath us.
      if (output.length === outputLength) {
        if (output === previousOutput) return { replace: false, text: "" }
        return reset(nextCommand, output, done)
      }
      // The engine truncated the head and prefixed an ellipsis, so earlier offsets no longer line up.
      if (output.startsWith("...\n\n") && !previousOutput.startsWith("...\n\n")) return reset(nextCommand, output, done)
      // Confirm this really is an append: the tail of what we last saw must still sit at the same
      // offset. If it does not, the output was rewritten rather than extended.
      const overlap = previousOutput.slice(-overlapProbeChars)
      if (output.slice(outputLength - overlap.length, outputLength) !== overlap) return reset(nextCommand, output, done)

      const normalized = consume(output.slice(outputLength), false)
      outputLength = output.length
      previousOutput = output
      if (visible) return { replace: false, text: normalized }
      // Still nothing but whitespace so far. Hold it back rather than opening the transcript with
      // blank lines, and emit the whole block at once as soon as real output arrives.
      pending.push(normalized)
      const combined = pending.join("")
      if (!combined.trim()) return { replace: false, text: "" }
      visible = true
      pending = []
      return { replace: true, text: `$ ${command}\n\n${combined}` }
    },
  }
}

export function createFrameCoalescer<T>(
  schedule: (callback: () => void) => number,
  cancel: (handle: number) => void,
  apply: (value: T) => void,
) {
  let frame: number | undefined
  let latest: T
  return {
    push(value: T, defer: boolean) {
      latest = value
      if (!defer) {
        if (frame !== undefined) cancel(frame)
        frame = undefined
        apply(latest)
        return
      }
      if (frame !== undefined) return
      frame = schedule(() => {
        frame = undefined
        apply(latest)
      })
    },
    dispose() {
      if (frame !== undefined) cancel(frame)
      frame = undefined
    },
  }
}

function ShellOutput(props: { command: string; output: string; running: boolean }) {
  const [copied, setCopied] = createSignal(false)
  const [renderRevision, setRenderRevision] = createSignal(0)
  let viewport!: HTMLPreElement
  let mounted = false
  let savedTop = 0
  let following = true
  const stream = createShellTranscriptStream()
  const normalizer = createFrameCoalescer(
    requestAnimationFrame,
    cancelAnimationFrame,
    ({ command, output, running }: { command: string; output: string; running: boolean }) => {
      const update = stream.update(command, output, !running)
      if (!mounted) return
      if (update.replace) viewport.textContent = update.text
      else if (update.text) {
        const node = viewport.firstChild
        if (node?.nodeType === Node.TEXT_NODE) (node as Text).appendData(update.text)
        else viewport.append(update.text)
      }
      if (update.replace || update.text) setRenderRevision((value) => value + 1)
    },
  )
  onMount(() => {
    mounted = true
    normalizer.push({ command: props.command, output: props.output, running: props.running }, false)
  })
  createEffect(
    on(
      () => [props.command, props.output, props.running] as const,
      ([command, output, running]) => {
        normalizer.push({ command, output, running }, running)
      },
      { defer: true },
    ),
  )
  onCleanup(() => normalizer.dispose())
  const copy = async () => {
    await navigator.clipboard.writeText(shellTranscript(props.command, props.output))
    setCopied(true)
    setTimeout(() => setCopied(false), copiedFeedbackMs)
  }
  createEffect(
    on(
      renderRevision,
      () => {
        const top = savedTop
        const follow = following
        queueMicrotask(() => {
          viewport.scrollTop = shellScrollTarget(top, follow, viewport.scrollHeight)
        })
      },
    ),
  )
  return (
    <div class="group/shell relative overflow-hidden rounded-[6px] border-[0.5px] border-edge">
      <button
        title={t("drift.shell.copyOutput")}
        class="absolute top-1 right-1 z-10 flex size-6 items-center justify-center rounded text-ink-faint opacity-0 transition-opacity group-focus-within/shell:opacity-100 group-hover/shell:opacity-100 hover:bg-raised hover:text-ink"
        onClick={() => void copy()}
      >
        <Show when={copied()} fallback={<IconCopy class="size-3.5" />}>
          <IconCheck class="size-3.5" />
        </Show>
      </button>
      <pre
        ref={viewport}
        class="shell-output code-display max-h-60 overflow-x-hidden overflow-y-auto p-3 pr-10 font-mono leading-[1.5] text-ink"
        role="region"
        aria-label={t("drift.shell.output")}
        tabIndex={0}
        onScroll={(event) => {
          savedTop = event.currentTarget.scrollTop
          following = shellAtBottom(savedTop, event.currentTarget.clientHeight, event.currentTarget.scrollHeight)
        }}
      >
      </pre>
    </div>
  )
}

export function shellAtBottom(scrollTop: number, clientHeight: number, scrollHeight: number) {
  return scrollHeight - clientHeight - scrollTop <= bottomSlopPx
}

export function shellScrollTarget(savedTop: number, following: boolean, scrollHeight: number) {
  return following ? scrollHeight : savedTop
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

export function parseDiff(diff: string): DiffRow[] {
  const rows: DiffRow[] = []
  let oldLine = 0
  let newLine = 0
  let oldRemaining = 0
  let newRemaining = 0
  let inHunk = false
  const lines = diff.split("\n")
  if (lines.at(-1) === "") lines.pop()
  for (const line of lines) {
    const hunk = line.match(/^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/)
    if (hunk) {
      oldLine = Number(hunk[1])
      oldRemaining = hunk[2] === undefined ? 1 : Number(hunk[2])
      newLine = Number(hunk[3])
      newRemaining = hunk[4] === undefined ? 1 : Number(hunk[4])
      inHunk = true
      if (rows.length) rows.push({ kind: "gap", text: "" })
      continue
    }
    if (!inHunk) continue
    if (line.startsWith("\\")) continue
    if (line.startsWith("+")) {
      rows.push({ kind: "add", line: newLine++, text: line.slice(1) })
      newRemaining--
    } else if (line.startsWith("-")) {
      rows.push({ kind: "del", line: oldLine++, text: line.slice(1) })
      oldRemaining--
    } else if (line.startsWith(" ")) {
      rows.push({ kind: "ctx", line: newLine, text: line.slice(1) })
      oldLine++
      newLine++
      oldRemaining--
      newRemaining--
    }
    if (oldRemaining <= 0 && newRemaining <= 0) inHunk = false
  }
  return rows
}

function DiffPanel(props: { diff: string; lang: string; bare?: boolean }) {
  const rows = createMemo(() => parseDiff(props.diff))
  const source = createMemo(() => ({
    code: rows().map((row) => row.text).join("\n"),
    lang: props.lang,
    theme: syntaxTheme(),
  }))
  const [tokens, setTokens] = createSignal<SyntaxToken[][]>([])
  let highlighted: ReturnType<typeof source> | undefined
  let request = 0
  createEffect(() => {
    const next = source()
    const current = ++request
    highlighted = undefined
    setTokens([])
    void codeTokens(next.code, next.lang).then((result) => {
      if (current !== request) return
      highlighted = next
      setTokens(result)
    })
  })
  const visibleTokens = () => (highlighted === source() ? tokens() : [])
  return (
    <div class="diff-view overflow-hidden" classList={{ "rounded-lg border border-edge": !props.bare }}>
      <div class="max-h-80 overflow-auto py-1 font-mono leading-relaxed">
        <div classList={{ "w-full": diffWordWrap(), "w-max min-w-full": !diffWordWrap() }}>
          <For each={rows()}>
            {(row, index) => (
              <Show when={row.kind !== "gap"} fallback={<div class="my-1 border-t border-dashed border-edge" />}>
                <div
                  class="flex border-l-2"
                  classList={{
                    "bg-ok/10": diffIndicator() === "background" && row.kind === "add",
                    "bg-danger/10": diffIndicator() === "background" && row.kind === "del",
                    "border-l-ok": diffIndicator() === "bars" && row.kind === "add",
                    "border-l-danger": diffIndicator() === "bars" && row.kind === "del",
                    "border-l-transparent": diffIndicator() !== "bars" || row.kind === "ctx",
                  }}
                >
                  <Show when={diffLineNumbers()}>
                    <span class="w-10 shrink-0 pr-2 text-right text-ink-faint select-none">{row.line}</span>
                  </Show>
                  <Show when={diffIndicator() === "symbols"}>
                    <span
                      class="w-4 shrink-0 text-center text-ink-faint select-none"
                      classList={{ "text-ok": row.kind === "add", "text-danger": row.kind === "del" }}
                    >
                      {row.kind === "add" ? "+" : row.kind === "del" ? "-" : " "}
                    </span>
                  </Show>
                  <span
                    class="min-w-0 flex-1 pr-3 text-ink-muted"
                    classList={{
                      "whitespace-pre-wrap [overflow-wrap:anywhere]": diffWordWrap(),
                      "whitespace-pre": !diffWordWrap(),
                    }}
                  >
                    <Show
                      when={visibleTokens()[index()]?.length ? visibleTokens()[index()] : undefined}
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
    </div>
  )
}

function SyntaxTokenView(props: { token: SyntaxToken }) {
  const style = () => {
    const fontStyle = props.token.fontStyle ?? 0
    return {
      color: props.token.color,
      "font-style": fontStyle & fontStyleItalic ? "italic" : undefined,
      "font-weight": fontStyle & fontStyleBold ? "bold" : undefined,
      "text-decoration": fontStyle & fontStyleUnderline ? "underline" : undefined,
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
    if (props.file.type === "add") return t("drift.file.created")
    if (props.file.type === "delete") return t("drift.file.deleted")
    if (props.file.type === "move") return t("drift.file.moved")
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
  const label = () => `${t("settings.permissions.tool.read.title")} · ${props.parts.length}`
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
          <span class="size-1.5 shrink-0 rounded-full bg-warn" title={t("notification.permission.title")} />
        </Show>
        <Show
          when={!waiting() && running()}
          fallback={<span class="font-semibold text-ink">{t("settings.permissions.tool.read.title")}</span>}
        >
          <TextShimmer text={t("settings.permissions.tool.read.title")} class="font-semibold" />
        </Show>
        <span class="text-ink-faint">{label()}</span>
        <Show when={waiting()}>
          <span class="text-xs text-warn/90">{t("notification.permission.title")}</span>
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
  return value.length > maxInlineOutputChars ? value.slice(0, maxInlineOutputChars) + "\n..." : value
}
