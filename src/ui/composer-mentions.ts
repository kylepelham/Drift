import { createEffect, createSignal } from "solid-js"
import { createLatestOnly } from "../state/latest"

/** Matches a trailing `@path` mention at the caret. Shared so the reader and replacer cannot drift. */
export const mentionPattern = /(^|\s)@([\w./\\-]*)$/

const maxMentionResults = 8

export type MentionAutocompleteOptions = {
  /** The textarea, read lazily because refs are assigned after this runs. */
  area: () => HTMLTextAreaElement
  draft: () => string
  setDraft: (text: string) => void
  mentions: () => string[]
  setMentions: (mentions: string[]) => void
  /** Only search once a workspace is connected; there is nothing to match against otherwise. */
  ready: () => boolean
  findFiles: (query: string) => Promise<string[]>
  /** Inserting a mention changes the text length, so the textarea has to be re-measured. */
  resize: () => void
}

/**
 * Typing `@` opens a file search whose results are inserted at the caret.
 *
 * The search runs on every keystroke, so results are guarded: a slow response for an earlier query
 * must not replace the hits for what the user has typed since.
 */
export function createMentionAutocomplete(options: MentionAutocompleteOptions) {
  const [query, setQuery] = createSignal<string | null>(null)
  const [hits, setHits] = createSignal<string[]>([])
  const [cursor, setCursor] = createSignal(0)
  const search = createLatestOnly()

  /** Index of the highlighted hit, clamped in case the list shrank under the cursor. */
  const activeIndex = () => Math.min(cursor(), hits().length - 1)

  /** Re-reads the text before the caret to decide whether a mention is being typed. */
  function refresh() {
    const area = options.area()
    const caret = area.selectionEnd ?? options.draft().length
    const match = options.draft().slice(0, caret).match(mentionPattern)
    setQuery(match ? match[2] : null)
  }

  createEffect(() => {
    const current = query()
    if (current === null || !options.ready()) {
      setHits([])
      return
    }
    const token = search.begin()
    void options.findFiles(current).then((found) => {
      if (!search.isCurrent(token)) return
      setHits(found.map((hit) => hit.replaceAll("\\", "/")).slice(0, maxMentionResults))
      setCursor(0)
    })
  })

  /** Replaces the partial `@…` at the caret with the chosen path and moves the caret past it. */
  function pick(path: string) {
    const area = options.area()
    const caret = area.selectionEnd ?? options.draft().length
    const before = options.draft().slice(0, caret)
    const match = before.match(mentionPattern)
    if (!match) return
    const start = caret - match[2].length - 1
    options.setDraft(options.draft().slice(0, start) + "@" + path + " " + options.draft().slice(caret))
    options.setMentions([...new Set([...options.mentions(), path])])
    setQuery(null)
    queueMicrotask(() => {
      options.resize()
      area.focus()
      // Past the inserted "@path " - the path plus the leading "@" and the trailing space.
      const position = start + path.length + 2
      area.setSelectionRange(position, position)
    })
  }

  /** Returns true when the key was consumed by the popover. */
  function handleKey(event: KeyboardEvent) {
    if (event.key === "ArrowDown") setCursor(Math.min(cursor() + 1, hits().length - 1))
    else if (event.key === "ArrowUp") setCursor(Math.max(cursor() - 1, 0))
    else if (event.key === "Escape") setQuery(null)
    else if (event.key === "Enter" || event.key === "Tab") pick(hits()[activeIndex()])
    else return false
    event.preventDefault()
    return true
  }

  /** True when the popover is showing results and should receive arrow/enter keys. */
  const open = () => query() !== null && hits().length > 0

  return { query, setQuery, hits, cursor, setCursor, activeIndex, open, refresh, pick, handleKey }
}

/**
 * Builds file-part payloads for the `@path` mentions that survived into the submitted text.
 *
 * A mention the user deleted before sending is skipped, which is why each path is looked up in the
 * final text rather than trusted from the draft's mention list alone.
 */
export function mentionFiles(text: string, paths: string[], root: string) {
  const directory = root.replaceAll("\\", "/").replace(/\/+$/, "")
  return paths.flatMap((path) => {
    const value = "@" + path
    const start = text.indexOf(value)
    if (start < 0 || !directory) return []
    const absolute = `${directory}/${path}`
    return [
      {
        mime: "text/plain",
        filename: path.split("/").pop(),
        url: "file:///" + encodeURI(absolute),
        source: { type: "file" as const, path: absolute, text: { value, start, end: start + value.length } },
      },
    ]
  })
}
