import type { AssistantMessage, Part, ToolPart, UserMessage } from "@opencode-ai/sdk/client"
import { createMemo, For, Match, onMount, Show, Switch } from "solid-js"
import { useEngine } from "../engine"
import { messageText, modelInfo, type MessageEntry } from "../engine/store"
import { emitMessageRendered } from "../plugins"
import { setRestoredDraft } from "../state/composer"
import { IconCopy, IconUndo } from "./icons"
import { Markdown } from "./markdown"
import { contextTools, ExploredGroup, FilePartView, PartView, partVisible } from "./parts"

export function MessageView(props: { entry: MessageEntry; footer?: boolean }) {
  onMount(() =>
    emitMessageRendered({
      sessionId: props.entry.info.sessionID,
      messageId: props.entry.info.id,
      role: props.entry.info.role,
    }),
  )
  return (
    <Show when={props.entry.info.role === "assistant"} fallback={<UserBubble entry={props.entry} />}>
      <AssistantFlow entry={props.entry} footer={props.footer} />
    </Show>
  )
}

function UserBubble(props: { entry: MessageEntry }) {
  const engine = useEngine()
  const info = () => props.entry.info as UserMessage
  const text = () => messageText(props.entry)
  const files = () => props.entry.parts.filter((part) => part.type === "file")
  const model = () => modelInfo(engine.state, info().model)?.name ?? info().model.modelID
  const time = () => new Date(info().time.created).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })
  const revert = async () => {
    await engine.actions.revert(info().sessionID, info().id)
    setRestoredDraft(text())
  }
  return (
    <Show when={text() || files().length > 0}>
      <div class="group flex flex-col items-end gap-1.5">
        <Show when={files().length > 0}>
          <div class="flex max-w-[85%] flex-wrap justify-end gap-1.5">
            <For each={files()}>{(file) => <FilePartView part={file} />}</For>
          </div>
        </Show>
        <Show when={text()}>
          <div class="max-w-[85%] rounded-lg border border-edge bg-surface px-3 py-1.5">
            <Markdown text={text()} done />
          </div>
        </Show>
        <div class="flex items-center gap-2 text-[0.7rem] text-ink-faint opacity-0 transition-opacity select-none group-focus-within:opacity-100 group-hover:opacity-100">
          <span>{capitalize(info().agent)} · {model()} · {time()}</span>
          <button title="Revert to here" class="rounded p-0.5 hover:bg-raised hover:text-ink" onClick={() => void revert()}>
            <IconUndo class="size-3.5" />
          </button>
          <button
            title="Copy message"
            class="rounded p-0.5 hover:bg-raised hover:text-ink"
            onClick={() => void navigator.clipboard.writeText(text())}
          >
            <IconCopy class="size-3.5" />
          </button>
        </div>
      </div>
    </Show>
  )
}

const capitalize = (value: string) => value.charAt(0).toUpperCase() + value.slice(1)

type PartGroup = { key: string; explored: ToolPart[] } | { key: string; part: Part }

function groupParts(parts: Part[]): PartGroup[] {
  const groups: PartGroup[] = []
  for (const part of parts) {
    if (part.type === "tool" && contextTools.has(part.tool)) {
      const last = groups.at(-1)
      if (last && "explored" in last) last.explored.push(part)
      else groups.push({ key: `explored:${part.id}`, explored: [part] })
      continue
    }
    groups.push({ key: part.id, part })
  }
  return groups
}

function AssistantFlow(props: { entry: MessageEntry; footer?: boolean }) {
  const info = () => props.entry.info as AssistantMessage
  const groups = createMemo(() => groupParts(props.entry.parts))
  const visible = () => props.entry.parts.some(partVisible) || !!info().error
  return (
    <Show when={visible()}>
      <div class="group space-y-2.5">
      <For each={groups()}>
        {(group) => (
          <Switch>
            <Match when={"explored" in group && group}>{(g) => <ExploredGroup parts={g().explored} />}</Match>
            <Match when={"part" in group && group}>{(g) => <PartView part={g().part} />}</Match>
          </Switch>
        )}
      </For>
      <Show when={info().error}>
        {(error) => (
          <div class="rounded-lg border border-danger/40 bg-danger/10 px-3 py-2 text-sm text-danger">
            {errorText(error())}
          </div>
        )}
      </Show>
      <Show when={props.footer && info().time.completed}>
        <div class="flex items-center gap-3 text-[0.7rem] text-ink-faint opacity-0 transition-opacity duration-200 select-none group-hover:opacity-100">
          <span>{info().modelID}</span>
          <span>{formatTokens(info())}</span>
          <Show when={tokensPerSecond(info())}>{(rate) => <span>{rate()} tok/s</span>}</Show>
          <Show when={info().cost > 0}>
            <span>${info().cost.toFixed(3)}</span>
          </Show>
          <span>{formatDuration(info().time.completed! - info().time.created)}</span>
          <button
            title="Copy response"
            class="rounded p-0.5 hover:bg-raised hover:text-ink"
            onClick={() => void navigator.clipboard.writeText(messageText(props.entry))}
          >
            <IconCopy class="size-3.5" />
          </button>
        </div>
      </Show>
      </div>
    </Show>
  )
}

function errorText(error: { name: string; data?: unknown }) {
  const data = error.data as { message?: string } | undefined
  return data?.message ?? error.name
}

function formatDuration(ms: number) {
  const seconds = Math.max(1, Math.round(ms / 1000))
  if (seconds < 60) return `${seconds}s`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`
}

function tokensPerSecond(info: AssistantMessage) {
  const seconds = ((info.time.completed ?? 0) - info.time.created) / 1000
  const tokens = info.tokens.output + info.tokens.reasoning
  if (seconds <= 0 || tokens <= 0) return null
  return (tokens / seconds).toFixed(1)
}

function formatTokens(info: AssistantMessage) {
  const compact = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(1)}k` : `${n}`)
  return `${compact(info.tokens.input)} in / ${compact(info.tokens.output)} out`
}
