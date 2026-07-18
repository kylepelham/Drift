import type { AssistantMessage, Part, ToolPart } from "@opencode-ai/sdk/client"
import { createMemo, For, Match, Show, Switch } from "solid-js"
import type { MessageEntry } from "../engine/store"
import { Markdown } from "./markdown"
import { contextTools, ExploredGroup, PartView } from "./parts"

export function MessageView(props: { entry: MessageEntry; footer?: boolean }) {
  return (
    <Show when={props.entry.info.role === "assistant"} fallback={<UserBubble parts={props.entry.parts} />}>
      <AssistantFlow entry={props.entry} footer={props.footer} />
    </Show>
  )
}

function UserBubble(props: { parts: Part[] }) {
  const text = () =>
    props.parts
      .filter((part) => part.type === "text" && !part.synthetic)
      .map((part) => (part as { text: string }).text)
      .join("\n")
  return (
    <Show when={text()}>
      <div class="fade-up ml-auto max-w-[85%] rounded-xl rounded-br-sm border border-edge bg-raised px-4 py-2.5">
        <Markdown text={text()} done />
      </div>
    </Show>
  )
}

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
  return (
    <div class="fade-up group space-y-2.5">
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
        <div class="flex gap-3 text-[0.7rem] text-ink-faint opacity-0 transition-opacity duration-200 group-hover:opacity-100">
          <span>{info().modelID}</span>
          <span>{formatTokens(info())}</span>
          <Show when={info().cost > 0}>
            <span>${info().cost.toFixed(3)}</span>
          </Show>
        </div>
      </Show>
    </div>
  )
}

function errorText(error: { name: string; data?: unknown }) {
  const data = error.data as { message?: string } | undefined
  return data?.message ?? error.name
}

function formatTokens(info: AssistantMessage) {
  const compact = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(1)}k` : `${n}`)
  return `${compact(info.tokens.input)} in / ${compact(info.tokens.output)} out`
}
