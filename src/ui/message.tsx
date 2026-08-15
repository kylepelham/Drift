import type { AssistantMessage, Part, ToolPart, UserMessage } from "@opencode-ai/sdk/client"
import { createRenderEffect, createSignal, For, Match, onMount, Show, Switch } from "solid-js"
import { createStore, reconcile, unwrap } from "solid-js/store"
import { useEngine } from "../engine"
import { errorText } from "../engine/error"
import { messageText, modelInfo, sessionBusy, type MessageEntry } from "../engine/store"
import { emitMessageRendered } from "../plugins"
import { composerScope, draftFromMessage, setComposerDraft } from "../state/composer"
import { agentLabel, t } from "../state/i18n"
import { collapseCompaction, compactionCollapsed } from "../state/prefs"
import { IconCopy, IconUndo } from "./icons"
import { Markdown } from "./markdown"
import { Chevron } from "./controls"
import { contextTools, ExploredGroup, FilePartView, PartView, partVisible } from "./parts"

export function MessageView(props: { entry: MessageEntry; footer?: boolean; groups?: PartGroup[] }) {
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
      <Show when={summary()} fallback={<AssistantFlow entry={props.entry} footer={props.footer} groups={props.groups} />}>
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
  // Seed prompts carried into spawned threads are machine-written and keep full Markdown.
  const generated = () => props.entry.parts.some((part) => part.type === "text" && part.metadata?.generated === true)
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
              <Show
                when={!generated() && largeUserText(text())}
                fallback={<Markdown text={text()} done humanAuthored={!generated()} />}
              >
                <pre class="user-paste">{text()}</pre>
              </Show>
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

export function largeUserText(text: string) {
  return text.length >= 2000 || text.split("\n", 41).length > 40
}

export type PartGroup = { id: string; key: string; explored: ToolPart[] } | { id: string; key: string; part: Part }

export type PartGroupSlot = { id: string; value: PartGroup; revision?: () => number; update: (value: PartGroup) => void }

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

function assistantBoundary(entry: MessageEntry) {
  if (entry.info.role !== "assistant") return true
  const info = entry.info as AssistantMessage
  return !!info.summary || !!info.error || entry.parts.some((part) => part.type === "compaction")
}

export function assistantFlowContinues(previous: MessageEntry, next: MessageEntry) {
  return previous.info.role === "assistant" && next.info.role === "assistant" &&
    !assistantBoundary(previous) && !assistantBoundary(next)
}

export function groupAssistantEntries(entries: MessageEntry[]) {
  const result = new Map<string, PartGroup[]>()
  let previous: MessageEntry | undefined
  let trailing: Extract<PartGroup, { explored: ToolPart[] }> | undefined
  for (const entry of entries) {
    if (entry.info.role !== "assistant") {
      previous = entry
      trailing = undefined
      continue
    }
    if (!previous || !assistantFlowContinues(previous, entry)) trailing = undefined
    const groups: PartGroup[] = []
    for (const group of groupParts(entry.parts)) {
      if ("explored" in group && trailing) {
        trailing.explored.push(...group.explored)
        continue
      }
      groups.push(group)
      trailing = "explored" in group ? group : undefined
    }
    result.set(entry.info.id, groups)
    previous = entry
  }
  return result
}

function createPartGroupSlot(group: PartGroup): PartGroupSlot {
  const [value, setValue] = createStore(group)
  const [revision, setRevision] = createSignal(0)
  return {
    id: group.id,
    value,
    revision,
    update: (updated) => {
      setValue(reconcile(unwrap(updated)))
      // reconcile can update a nested source proxy without invalidating consumers of part.text.
      // An explicit revision preserves the mounted slot while guaranteeing those consumers rerun.
      setRevision((value) => value + 1)
    },
  }
}

export function updatePartGroupSlots(
  groups: PartGroup[],
  slots: Map<string, PartGroupSlot>,
  createSlot = createPartGroupSlot,
) {
  const previous = [...slots.values()]
  const exploredByPart = new Map<string, { index: number; slot: PartGroupSlot }>()
  previous.forEach((slot, index) => {
    if (!("explored" in slot.value)) return
    slot.value.explored.forEach((part) => exploredByPart.set(part.id, { index, slot }))
  })
  // A split can overlap one prior group more than once; its anchor-containing fragment owns the old mount.
  const reserved = new Map<string, number>()
  previous.forEach((slot) => {
    if (!("explored" in slot.value)) return
    const owner = groups.findIndex(
      (group) => "explored" in group && group.explored.some((part) => `explored:${part.id}` === slot.id),
    )
    if (owner !== -1) reserved.set(slot.id, owner)
  })
  const claimed = new Set<string>()
  const active = new Set<string>()
  const next = groups.map((input, index) => {
    const existing = "explored" in input
      ? input.explored.reduce<{ index: number; slot: PartGroupSlot } | undefined>((result, part) => {
          const candidate = exploredByPart.get(part.id)
          if (!candidate || claimed.has(candidate.slot.id)) return result
          const owner = reserved.get(candidate.slot.id)
          if (owner !== undefined && owner !== index) return result
          return !result || candidate.index < result.index ? candidate : result
        }, undefined)?.slot
      : slots.get(input.id)
    const group = existing && input.id !== existing.id
      ? { ...input, id: existing.id, key: existing.id }
      : input
    if (existing && "explored" in input) claimed.add(existing.id)
    active.add(group.id)
    if (existing) {
      existing.update(group)
      return existing
    }
    const slot = createSlot(group)
    slots.set(group.id, slot)
    return slot
  })
  for (const id of slots.keys()) if (!active.has(id)) slots.delete(id)
  for (const slot of next) {
    slots.delete(slot.id)
    slots.set(slot.id, slot)
  }
  return next
}

function AssistantFlow(props: { entry: MessageEntry; footer?: boolean; groups?: PartGroup[] }) {
  const engine = useEngine()
  const info = () => props.entry.info as AssistantMessage
  const slots = new Map<string, PartGroupSlot>()
  const [groups, setGroups] = createSignal<PartGroupSlot[]>([])
  createRenderEffect(() => setGroups(updatePartGroupSlots(props.groups ?? groupParts(props.entry.parts), slots)))
  const visible = () => groups().length > 0 || !!info().error || (!!props.footer && !!info().time.completed)
  const liveTextPartID = () => {
    if (info().time.completed || !sessionBusy(engine.state, info().sessionID)) return undefined
    return [...props.entry.parts]
      .reverse()
      .find((part) => part.type === "text" && !part.time?.end)?.id
  }
  return (
    <Show when={visible()}>
      <div class="group flex min-w-0 max-w-full flex-col gap-3">
        <For each={groups()}>
          {(group) => (
            <Switch>
              <Match when={"explored" in group.value && group.value}>
                {(explored) => <ExploredGroup parts={explored().explored} />}
              </Match>
              <Match when={"part" in group.value && group.value}>
                {(single) => (
                  <PartView
                    part={single().part}
                    revision={group.revision?.()}
                    responseID={`${info().id}:${single().part.id}`}
                    live={single().part.id === liveTextPartID()}
                  />
                )}
              </Match>
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
            <Show when={tokensPerSecond(props.entry)}>
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

function formatDuration(ms: number) {
  const seconds = Math.max(1, Math.round(ms / 1000))
  if (seconds < 60) return t("drift.message.duration.seconds", { seconds })
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return t("drift.message.duration.minutes", { minutes, seconds: seconds % 60 })
  return t("drift.message.duration.hours", { hours: Math.floor(minutes / 60), minutes: minutes % 60 })
}

// Only the spans the model spent generating text or reasoning count toward the rate; wall time
// also covers tool runs and subagent waits, which made the shown rate meaningless.
export function generationMs(entry: MessageEntry) {
  const info = entry.info as AssistantMessage
  let total = 0
  for (const part of entry.parts) {
    if (part.type !== "text" && part.type !== "reasoning") continue
    const time = (part as { time?: { start?: number; end?: number } }).time
    if (time?.start === undefined) continue
    const end = time.end ?? info.time.completed
    if (end) total += Math.max(0, end - time.start)
  }
  return total
}

export function tokensPerSecond(entry: MessageEntry) {
  const info = entry.info as AssistantMessage
  const elapsed = generationMs(entry) || (info.time.completed ?? 0) - info.time.created
  const tokens = info.tokens.output + info.tokens.reasoning
  if (elapsed <= 0 || tokens <= 0) return null
  return (tokens / (elapsed / 1000)).toFixed(1)
}

function formatTokens(info: AssistantMessage) {
  const compact = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(1)}k` : `${n}`)
  return t("drift.message.tokenCounts", {
    input: compact(info.tokens.input),
    output: compact(info.tokens.output),
  })
}
