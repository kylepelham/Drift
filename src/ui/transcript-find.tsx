import { createEffect, createMemo, createSignal, onCleanup, onMount, Show } from "solid-js"
import { t } from "../state/i18n"
import { onKeybind } from "../state/keybinds"
import { selectedSession } from "../state/selection"
import {
  reanchorMatch,
  stepMatch,
  totalMatches,
  transcriptMatches,
  type TranscriptMatch,
} from "../state/transcript-search"
import type { MessageEntry } from "../engine/store"
import { IconArrowDown, IconArrowUp, IconSearch, IconX } from "./icons"

// The bar belongs to the header while the matches belong to the transcript, so the query lives
// beside both. Only one conversation is open at a time, so a single value is enough.
const [open, setOpen] = createSignal(false)
const [query, setQuery] = createSignal("")
const [cursor, setCursor] = createSignal(-1)
const [matches, setMatches] = createSignal<TranscriptMatch[]>([])

export function transcriptFindOpen() {
  return open()
}

export function transcriptFindQuery() {
  return query()
}

/** The message the cursor currently sits on, which the timeline scrolls to and highlights. */
export function activeFindMessage() {
  const index = cursor()
  return index >= 0 ? matches()[index]?.messageId : undefined
}

export function findHighlightedMessages() {
  return matches()
}

export function openTranscriptFind() {
  setOpen(true)
}

export function closeTranscriptFind() {
  setOpen(false)
  setQuery("")
  setMatches([])
  setCursor(-1)
}

/**
 * Recomputes matches from the current transcript.
 *
 * Called by the timeline whenever its entries change, so results follow a streaming reply and any
 * older page that loads, and the cursor keeps pointing at the same message while they do.
 */
export function syncTranscriptMatches(entries: MessageEntry[]) {
  if (!open()) return
  const found = transcriptMatches(entries, query())
  const anchored = activeFindMessage()
  setMatches(found)
  setCursor(reanchorMatch(found, anchored, cursor()))
}

export function stepTranscriptFind(step: number) {
  setCursor((current) => stepMatch(current, matches().length, step))
}

export function TranscriptFindBar() {
  let input: HTMLInputElement | undefined

  onMount(() => {
    onKeybind("findInSession", () => {
      if (!selectedSession()) return
      setOpen(true)
      queueMicrotask(() => {
        input?.focus()
        input?.select()
      })
    })
  })

  // Switching conversations discards the search: its matches belong to a transcript that is gone.
  createEffect(() => {
    selectedSession()
    closeTranscriptFind()
  })

  createEffect(() => {
    if (!open()) return
    query()
    queueMicrotask(() => input?.focus())
  })
  onCleanup(closeTranscriptFind)

  const total = createMemo(() => totalMatches(matches()))
  const position = createMemo(() => {
    const index = cursor()
    if (index < 0 || !matches().length) return 0
    // Matches are counted per occurrence, so the label counts every occurrence before this message.
    return matches().slice(0, index).reduce((sum, match) => sum + match.count, 0) + 1
  })

  return (
    <Show when={open()}>
      <div class="pointer-events-auto flex items-center gap-1 rounded-md border border-edge bg-overlay px-1.5 py-0.5 shadow-lg shadow-black/20">
        <IconSearch class="size-3.5 shrink-0 text-ink-faint" />
        <input
          ref={input}
          class="w-40 bg-transparent py-0.5 text-xs text-ink outline-none placeholder:text-ink-faint"
          placeholder={t("drift.search.transcript.placeholder")}
          value={query()}
          onInput={(event) => {
            setQuery(event.currentTarget.value)
            setCursor(event.currentTarget.value ? 0 : -1)
          }}
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.preventDefault()
              closeTranscriptFind()
              return
            }
            if (event.key !== "Enter") return
            event.preventDefault()
            stepTranscriptFind(event.shiftKey ? -1 : 1)
          }}
        />
        <span class="shrink-0 tabular-nums text-[0.65rem] text-ink-faint">
          {query() ? `${position()}/${total()}` : ""}
        </span>
        <button
          class="flex size-5 shrink-0 items-center justify-center rounded text-ink-faint transition-colors hover:text-ink disabled:opacity-40"
          title={t("drift.search.previous")}
          disabled={!total()}
          onClick={() => stepTranscriptFind(-1)}
        >
          <IconArrowUp class="size-3" />
        </button>
        <button
          class="flex size-5 shrink-0 items-center justify-center rounded text-ink-faint transition-colors hover:text-ink disabled:opacity-40"
          title={t("drift.search.next")}
          disabled={!total()}
          onClick={() => stepTranscriptFind(1)}
        >
          <IconArrowDown class="size-3" />
        </button>
        <button
          class="flex size-5 shrink-0 items-center justify-center rounded text-ink-faint transition-colors hover:text-ink"
          title={t("drift.search.close")}
          onClick={closeTranscriptFind}
        >
          <IconX class="size-3" />
        </button>
      </div>
    </Show>
  )
}
