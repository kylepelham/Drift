import { createEffect, createMemo, createSignal, on, onCleanup, onMount, Show, untrack } from "solid-js"
import { t } from "../state/i18n"
import { onKeybind } from "../state/keybinds"
import { selectedSession } from "../state/selection"
import {
  occurrenceAt,
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
// What the search actually runs on. It trails the input so a fast typist does not pay for a full
// transcript scan, a scroll jump, and a highlight repaint on every keystroke.
const [needle, setNeedle] = createSignal("")
const [cursor, setCursor] = createSignal(-1)
const [matches, setMatches] = createSignal<TranscriptMatch[]>([])

export const findDebounceMs = 150
let debounce: ReturnType<typeof setTimeout> | undefined
let syncedNeedle = ""

function settle(value: string) {
  if (debounce !== undefined) clearTimeout(debounce)
  debounce = undefined
  // Clearing is instant so Escape and the clear button never leave stale highlights behind.
  if (!value.trim()) {
    setNeedle("")
    return
  }
  debounce = setTimeout(() => setNeedle(value), findDebounceMs)
}

function flushPending() {
  if (debounce === undefined) return
  clearTimeout(debounce)
  debounce = undefined
  setNeedle(query())
}

export function transcriptFindOpen() {
  return open()
}

export function transcriptFindNeedle() {
  return open() ? needle().trim() : ""
}

export function transcriptFindCursor() {
  return cursor()
}

/** The occurrence the cursor sits on; the timeline scrolls to it and paints it distinctly. */
export function activeFindOccurrence() {
  return occurrenceAt(matches(), cursor())
}

/** The message the cursor currently sits on, which the timeline scrolls to and highlights. */
export function activeFindMessage() {
  return activeFindOccurrence()?.messageId
}

export function openTranscriptFind() {
  setOpen(true)
}

export function closeTranscriptFind() {
  if (debounce !== undefined) clearTimeout(debounce)
  debounce = undefined
  syncedNeedle = ""
  setOpen(false)
  setQuery("")
  setNeedle("")
  setMatches([])
  setCursor(-1)
}

/**
 * Recomputes matches from the current transcript.
 *
 * Called by the timeline whenever its entries or the settled query change, so results follow a
 * streaming reply and any older page that loads. A new query starts from its first occurrence; a
 * transcript change under the same query keeps the cursor on the occurrence it was already at.
 *
 * The anchor reads run untracked: this executes inside the caller's effect, and reading the very
 * signals it is about to write would make each run retrigger the next forever, until Solid's
 * update guard kills the effect and search silently reports nothing.
 */
export function syncTranscriptMatches(entries: MessageEntry[]) {
  if (!open()) return
  const value = needle()
  const found = transcriptMatches(entries, value)
  untrack(() => {
    const fresh = value !== syncedNeedle
    syncedNeedle = value
    const anchor = fresh ? undefined : activeFindOccurrence()
    setMatches(found)
    setCursor(fresh ? (found.length ? 0 : -1) : reanchorMatch(found, anchor, cursor()))
  })
}

export function stepTranscriptFind(step: number) {
  setCursor((current) => stepMatch(current, totalMatches(matches()), step))
}

const FIND_HIGHLIGHT = "drift-find"
const FIND_ACTIVE_HIGHLIGHT = "drift-find-active"

function highlightRegistry() {
  return typeof Highlight === "undefined" ? undefined : CSS.highlights
}

export function clearFindHighlights() {
  const registry = highlightRegistry()
  registry?.delete(FIND_HIGHLIGHT)
  registry?.delete(FIND_ACTIVE_HIGHLIGHT)
}

/**
 * Paints every occurrence of the needle inside the mounted rows and returns the active one.
 *
 * The timeline is virtualized, so this only ever walks the handful of rows that exist; scrolling
 * repaints as rows mount. Highlights are ranges registered with the CSS highlight registry rather
 * than injected markup, so streamed markdown is never mutated. An occurrence whose text is split
 * across markdown elements is counted by the data model but cannot be painted; that only costs the
 * paint, not the position or navigation.
 */
export function paintFindHighlights(container: HTMLElement): Range | undefined {
  const registry = highlightRegistry()
  if (!registry) return
  const value = transcriptFindNeedle().toLowerCase()
  if (!value) {
    clearFindHighlights()
    return
  }
  const active = activeFindOccurrence()
  const ranges: Range[] = []
  let activeRange: Range | undefined
  for (const row of container.querySelectorAll<HTMLElement>("[data-mid]")) {
    const isActiveRow = row.dataset.mid === active?.messageId
    let seen = 0
    const walker = document.createTreeWalker(row, NodeFilter.SHOW_TEXT)
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const text = (node.nodeValue ?? "").toLowerCase()
      for (let at = text.indexOf(value); at !== -1; at = text.indexOf(value, at + value.length)) {
        const range = new Range()
        range.setStart(node, at)
        range.setEnd(node, at + value.length)
        ranges.push(range)
        if (isActiveRow && seen === active?.index) activeRange = range
        seen++
      }
    }
  }
  registry.set(FIND_HIGHLIGHT, new Highlight(...ranges))
  if (activeRange) registry.set(FIND_ACTIVE_HIGHLIGHT, new Highlight(activeRange))
  else registry.delete(FIND_ACTIVE_HIGHLIGHT)
  return activeRange
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

  createEffect(on(open, (value) => {
    if (value) queueMicrotask(() => input?.focus())
  }))
  onCleanup(closeTranscriptFind)

  const total = createMemo(() => totalMatches(matches()))
  const position = createMemo(() => (cursor() >= 0 ? cursor() + 1 : 0))

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
            settle(event.currentTarget.value)
          }}
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.preventDefault()
              closeTranscriptFind()
              return
            }
            if (event.key !== "Enter") return
            event.preventDefault()
            flushPending()
            stepTranscriptFind(event.shiftKey ? -1 : 1)
          }}
        />
        <span class="shrink-0 tabular-nums text-[0.65rem] text-ink-faint">
          {needle().trim() ? `${position()}/${total()}` : ""}
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
