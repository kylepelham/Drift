import type { AssistantMessage, Part, ToolPart, UserMessage } from "@opencode-ai/sdk/client"
import { createRenderEffect, createSignal, For, Match, onMount, Show, Switch } from "solid-js"
import { createStore, reconcile } from "solid-js/store"
import { useEngine } from "../engine"
import { errorText } from "../engine/error"
import { messageText, modelInfo, type MessageEntry } from "../engine/store"
import { emitMessageRendered } from "../plugins"
import { composerScope, draftFromMessage, setComposerDraft } from "../state/composer"
import { agentLabel, t } from "../state/i18n"
import { collapseCompaction, compactionCollapsed } from "../state/prefs"
import { IconCopy, IconUndo } from "./icons"
import { Markdown } from "./markdown"
import { Chevron, contextTools, ExploredGroup, FilePartView, PartView, partVisible } from "./parts"

export function MessageView(props: { entry: MessageEntry; footer?: boolean }) {
  onMount(() =>
    emitMessageRendered({
      sessionId: props.entry.info.sessionID,
      messageId: props.entry.info.id,
      role: props.entry.info.role,
    }),
  )
  const summary = () => (props.entry.info as AssistantMessage).summary && collapseCompaction()
  return (
    <Show when={props.entry.info.role === "assistant"} fallback={<UserBubble entry={props.entry} />}>
      <Show when={summary()} fallback={<AssistantFlow entry={props.entry} footer={props.footer} />}>
        <CompactionSummary entry={props.entry} footer={props.footer} />
      </Show>
    </Show>
  )
}

export function messageVisible(entry: MessageEntry) {
  if (entry.info.role === "user")
    return !!messageText(entry) || entry.parts.some((part) => part.type === "file" || part.type === "compaction")
  const info = entry.info as AssistantMessage
  if (info.summary && collapseCompaction()) return true
  return entry.parts.some(partVisible) || !!info.error
}

function CompactionSummary(props: { entry: MessageEntry; footer?: boolean }) {
  const [open, setOpen] = createSignal(!compactionCollapsed())
  return (
    <div class="min-w-0 max-w-full">
      <button
        class="flex w-full items-center gap-3 py-1 text-xs text-ink-faint transition-colors hover:text-ink-muted"
        aria-expanded={open()}
        onClick={() => setOpen(!open())}
      >
        <div class="h-px flex-1 bg-edge" />
        <span class="flex items-center gap-1.5">
          <Chevron open={open()} />
          {t("drift.message.compactedSummary")}
        </span>
        <div class="h-px flex-1 bg-edge" />
      </button>
      <Show when={open()}>
        <div class="mt-2 cursor-pointer border-l-2 border-edge pl-3" onClick={() => setOpen(false)}>
          <AssistantFlow entry={props.entry} footer={props.footer} />
        </div>
      </Show>
    </div>
  )
}

export function compactionParts(entry: MessageEntry) {
  return entry.parts.filter((part) => part.type === "compaction")
}

function UserBubble(props: { entry: MessageEntry }) {
  const engine = useEngine()
  const info = () => props.entry.info as UserMessage
  const text = () => messageText(props.entry)
  const files = () => props.entry.parts.filter((part) => part.type === "file")
  const compactions = () => compactionParts(props.entry)
  const model = () => modelInfo(engine.state, info().model)?.name ?? info().model.modelID
  const time = () => new Date(info().time.created).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })
  const revert = async () => {
    const restored = draftFromMessage(props.entry)
    if (await engine.actions.revert(info().sessionID, info().id))
      setComposerDraft(composerScope(info().sessionID), restored)
  }
  return (
    <>
      <Show when={text() || files().length > 0}>
        <div class="group flex flex-col items-end gap-1.5">
          <Show when={files().length > 0}>
            <div class="flex max-w-[85%] flex-wrap justify-end gap-1.5">
              <For each={files()}>{(file) => <FilePartView part={file} />}</For>
            </div>
          </Show>
          <Show when={text()}>
            <div class="max-w-[85%] rounded-lg border border-edge bg-surface px-3 py-1.5">
              <Markdown text={text()} done literalBackslashes />
            </div>
          </Show>
          <div class="flex items-center gap-2 text-[0.7rem] text-ink-faint opacity-0 transition-opacity select-none group-focus-within:opacity-100 group-hover:opacity-100">
            <span>{agentLabel(info().agent)} · {model()} · {time()}</span>
            <button title={t("drift.message.revertHere")} class="rounded p-0.5 hover:bg-raised hover:text-ink" onClick={() => void revert()}>
              <IconUndo class="size-3.5" />
            </button>
            <button
              title={t("drift.message.copy")}
              class="rounded p-0.5 hover:bg-raised hover:text-ink"
              onClick={() => void navigator.clipboard.writeText(text())}
            >
              <IconCopy class="size-3.5" />
            </button>
          </div>
        </div>
      </Show>
      <For each={compactions()}>{(part) => <PartView part={part} />}</For>
    </>
  )
}

export type PartGroup = { id: string; key: string; explored: ToolPart[] } | { id: string; key: string; part: Part }

export type PartGroupSlot = { id: string; value: PartGroup; update: (value: PartGroup) => void }

export function groupParts(parts: Part[]): PartGroup[] {
  const groups: PartGroup[] = []
  for (const part of parts) {
    if (!partVisible(part)) continue
    if (part.type === "tool" && contextTools.has(part.tool)) {
      const last = groups.at(-1)
      if (last && "explored" in last) last.explored.push(part)
      else groups.push({ id: `explored:${part.id}`, key: `explored:${part.id}`, explored: [part] })
      continue
    }
    groups.push({ id: part.id, key: part.id, part })
  }
  return groups
}

function createPartGroupSlot(group: PartGroup): PartGroupSlot {
  const [value, setValue] = createStore(group)
  return { id: group.id, value, update: (updated) => setValue(reconcile(updated)) }
}

export function updatePartGroupSlots(
  groups: PartGroup[],
  slots: Map<string, PartGroupSlot>,
  createSlot = createPartGroupSlot,
) {
  const active = new Set<string>()
  const next = groups.map((group) => {
    active.add(group.id)
    const existing = slots.get(group.id)
    if (existing) {
      existing.update(group)
      return existing
    }
    const slot = createSlot(group)
    slots.set(group.id, slot)
    return slot
  })
  for (const id of slots.keys()) if (!active.has(id)) slots.delete(id)
  return next
}

function AssistantFlow(props: { entry: MessageEntry; footer?: boolean }) {
  const info = () => props.entry.info as AssistantMessage
  const slots = new Map<string, PartGroupSlot>()
  const [groups, setGroups] = createSignal<PartGroupSlot[]>([])
  createRenderEffect(() => setGroups(updatePartGroupSlots(groupParts(props.entry.parts), slots)))
  const visible = () => props.entry.parts.some(partVisible) || !!info().error
  return (
    <Show when={visible()}>
      <div class="group flex min-w-0 max-w-full flex-col gap-3">
        <For each={groups()}>
          {(group) => (
            <Switch>
              <Match when={"explored" in group.value && group.value}>{(g) => <ExploredGroup parts={g().explored} />}</Match>
              <Match when={"part" in group.value && group.value}>{(g) => <PartView part={g().part} />}</Match>
            </Switch>
          )}
        </For>
        <Show when={info().error}>
          {(error) => (
            <Show
              when={error().name !== "MessageAbortedError"}
              fallback={
                <div class="flex items-center gap-3 py-1 text-xs text-ink-faint" role="status">
                  <div class="h-px flex-1 bg-edge" />
                  {t("drift.message.interrupted")}
                  <div class="h-px flex-1 bg-edge" />
                </div>
              }
            >
              <div class="rounded-lg border border-danger/40 bg-danger/10 px-3 py-2 text-sm break-words text-danger" role="alert">
                {errorText(error())}
              </div>
            </Show>
          )}
        </Show>
        <Show when={props.footer && info().time.completed}>
          <div class="flex items-center gap-3 text-[0.7rem] text-ink-faint opacity-0 transition-opacity duration-200 select-none group-hover:opacity-100">
            <span>{info().modelID}</span>
            <span>{formatTokens(info())}</span>
            <Show when={tokensPerSecond(info())}>
              {(rate) => <span>{t("drift.message.tokensPerSecond", { rate: rate() })}</span>}
            </Show>
            <Show when={info().cost > 0}>
              <span>${info().cost.toFixed(3)}</span>
            </Show>
            <span>{formatDuration(info().time.completed! - info().time.created)}</span>
            <button
              title={t("drift.message.copyResponse")}
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

export { errorText, unwrapErrorMessage } from "../engine/error"

function formatDuration(ms: number) {
  const seconds = Math.max(1, Math.round(ms / 1000))
  if (seconds < 60) return t("drift.message.duration.seconds", { seconds })
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return t("drift.message.duration.minutes", { minutes, seconds: seconds % 60 })
  return t("drift.message.duration.hours", { hours: Math.floor(minutes / 60), minutes: minutes % 60 })
}

function tokensPerSecond(info: AssistantMessage) {
  const seconds = ((info.time.completed ?? 0) - info.time.created) / 1000
  const tokens = info.tokens.output + info.tokens.reasoning
  if (seconds <= 0 || tokens <= 0) return null
  return (tokens / seconds).toFixed(1)
}

function formatTokens(info: AssistantMessage) {
  const compact = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(1)}k` : `${n}`)
  return t("drift.message.tokenCounts", {
    input: compact(info.tokens.input),
    output: compact(info.tokens.output),
  })
}
