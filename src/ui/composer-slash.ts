import { createMemo, createSignal } from "solid-js"
import type { Engine } from "../engine"
import { parseSlash, runSlash, slashItem, slashItems, slashPresets, type SlashItem, type SlashPreset } from "./slash"

export type SlashMenuOptions = {
  engine: Engine
  /** The textarea, read lazily because refs are assigned after this runs. */
  area: () => HTMLTextAreaElement
  draft: () => string
  setDraft: (text: string) => void
  /** Running a command clears the draft, so the textarea has to be re-measured. */
  resize: () => void
}

/**
 * Typing `/` opens a command menu over the composer.
 *
 * There are two levels. While the user is still typing the command name the menu lists matching
 * commands. Once a separator has been typed the command is fixed and the menu lists that command's
 * argument presets instead, so `cursor` indexes whichever list is currently showing.
 */
export function createSlashMenu(options: SlashMenuOptions) {
  // Set when the user dismisses the menu with Escape or navigates history; cleared on the next edit
  // so the menu does not immediately reopen for text that still starts with "/".
  const [dismissed, setDismissed] = createSignal(false)
  const [cursor, setCursor] = createSignal(0)

  const parsed = () => (dismissed() ? null : parseSlash(options.draft()))

  const matches = createMemo<SlashItem[]>(() => {
    const current = parsed()
    return current ? slashItems(options.engine, current.query) : []
  })

  /** The command the draft has settled on, once a separator means the name is no longer being typed. */
  const argumentItem = createMemo(() => {
    const current = parsed()
    return current?.separated ? slashItem(options.engine, current.query) : undefined
  })

  const argumentPresets = createMemo(() => {
    const current = parsed()
    const item = argumentItem()
    return current && item ? slashPresets(item, current.args) : []
  })

  const activeMatchIndex = () => Math.min(cursor(), matches().length - 1)
  const activePresetIndex = () => Math.min(cursor(), argumentPresets().length - 1)

  /** Runs a command, or fills in its name and waits when it still needs arguments. */
  async function pick(item: SlashItem) {
    const args = parsed()?.args ?? ""
    if ((item.requiredArgs || item.presets?.length) && !args) {
      options.setDraft(`/${item.name} `)
      setCursor(0)
      queueMicrotask(() => options.area().focus())
      return
    }
    options.setDraft("")
    options.resize()
    await runSlash(options.engine, item, args)
  }

  /** Runs a preset, or fills it into the draft when the preset is meant to be edited first. */
  async function pickPreset(item: SlashItem, preset: SlashPreset) {
    if (!preset.execute) {
      options.setDraft(`/${item.name} ${preset.value}`)
      setCursor(0)
      queueMicrotask(() => {
        options.resize()
        options.area().focus()
      })
      return
    }
    options.setDraft("")
    options.resize()
    await runSlash(options.engine, item, preset.value.trim())
  }

  /** Returns true when the key was consumed by the menu. */
  function handleKey(event: KeyboardEvent) {
    // Shift+Enter inserts a newline rather than accepting the highlighted entry.
    if (event.key === "Enter" && event.shiftKey) return false
    const item = argumentItem()
    const presets = argumentPresets()
    const accept = event.key === "Enter" || event.key === "Tab"
    // When a command is fixed the menu shows its presets, but a command with no presets still
    // occupies one row so the cursor has something to sit on.
    const count = item ? Math.max(1, presets.length) : matches().length

    if (event.key === "ArrowDown") setCursor(Math.min(cursor() + 1, count - 1))
    else if (event.key === "ArrowUp") setCursor(Math.max(cursor() - 1, 0))
    else if (event.key === "Escape") setDismissed(true)
    else if (!accept) return false
    else if (!item) void pick(matches()[activeMatchIndex()])
    else if (presets.length) void pickPreset(item, presets[activePresetIndex()])
    // With arguments already typed, or a command that needs none, accepting runs it directly.
    else if (parsed()?.args || !item.requiredArgs) void pick(item)
    else return false

    event.preventDefault()
    return true
  }

  /** True when the menu is showing entries and should receive arrow/enter keys. */
  const open = () => matches().length > 0

  return {
    parsed,
    matches,
    argumentItem,
    argumentPresets,
    cursor,
    setCursor,
    activeMatchIndex,
    activePresetIndex,
    dismissed,
    setDismissed,
    open,
    pick,
    pickPreset,
    handleKey,
  }
}
