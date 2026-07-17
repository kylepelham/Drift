import type { AssistantMessage, Part } from "@opencode-ai/sdk/client"
import { For, Show } from "solid-js"
import type { MessageEntry } from "../engine/store"
import { Markdown } from "./markdown"
import { PartView } from "./parts"

export function MessageView(props: { entry: MessageEntry }) {
  return (
    <Show when={props.entry.info.role === "assistant"} fallback={<UserBubble parts={props.entry.parts} />}>
      <AssistantFlow entry={props.entry} />
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

function AssistantFlow(props: { entry: MessageEntry }) {
  const info = () => props.entry.info as AssistantMessage
  return (
    <div class="fade-up space-y-2.5">
      <For each={props.entry.parts}>{(part) => <PartView part={part} />}</For>
      <Show when={info().error}>
        {(error) => (
          <div class="rounded-lg border border-danger/40 bg-danger/10 px-3 py-2 text-sm text-danger">
            {errorText(error())}
          </div>
        )}
      </Show>
      <Show when={info().time.completed}>
        <div class="flex gap-3 text-[0.7rem] text-ink-faint">
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
