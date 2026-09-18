import { createEffect, createMemo, createSignal, createUniqueId, on } from "solid-js"
import type { Engine } from "../engine"
import { isDesktopShell } from "../shell"
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
  const id = `slash-${createUniqueId()}`
  // Set when the user dismisses the menu with Escape or navigates history; cleared on the next edit
  // so the menu does not immediately reopen for text that still starts with "/".
  const [dismissed, setDismissed] = createSignal(false)
  const [cursor, setCursor] = createSignal(0)
  const [expandedArgument, setExpandedArgument] = createSignal<string>()
  createEffect(on(options.draft, () => {
    setCursor(0)
    setExpandedArgument(undefined)
  }))

  const parsed = () => (dismissed() ? null : parseSlash(options.draft()))
  let slashOpen = false
  createEffect(() => {
    const open = parsed() !== null
    if (open && !slashOpen && !isDesktopShell()) void options.engine.refreshRuntimeMetadata().catch(() => undefined)
    slashOpen = open
  })

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
    // Once a subcommand is completed, keep its help visible while the user supplies its target.
    if (current?.args && (/\s/.test(current.args) || /\s$/.test(options.draft()))) return []
    return current && item ? slashPresets(item, current.args) : []
  })

  const argumentHelp = createMemo(() => {
    const item = argumentItem()
    const first = parsed()?.args.split(/\s/)[0]
    const preset = item && slashPresets(item, "").find((preset) => preset.value.trim().toLowerCase() === first?.toLowerCase())
    if (preset) return { usage: preset.usage, description: preset.description }
    if (first && item?.presets?.length) return { usage: undefined, description: undefined }
    return { usage: item?.usage, description: item?.description }
  })

  const activeMatchIndex = () => Math.min(cursor(), matches().length - 1)
  const activePresetIndex = () => Math.min(cursor(), argumentPresets().length - 1)

  function toggleArgumentHelp(preset: SlashPreset) {
    setExpandedArgument((current) => current === preset.value ? undefined : preset.value)
  }

  function complete(item: SlashItem, preset?: SlashPreset) {
    const text = `/${item.name} ${preset?.value ?? parsed()?.args ?? ""}`
    options.setDraft(text)
    setCursor(0)
    queueMicrotask(() => {
      options.resize()
      options.area().focus()
      options.area().setSelectionRange(text.length, text.length)
    })
  }

  async function execute(item: SlashItem, args: string) {
    try {
      await runSlash(options.engine, item, args)
    } catch (error) {
      options.engine.actions.notice({
        title: "Command failed",
        message: error instanceof Error ? error.message : String(error),
        variant: "error",
      })
    }
  }

  /** Runs a command, or fills in its name and waits when it still needs arguments. */
  async function pick(item: SlashItem) {
    const args = parsed()?.args ?? ""
    if ((item.requiredArgs || item.presets?.length || item.usage) && !args) {
      complete(item)
      return
    }
    options.setDraft("")
    options.resize()
    await execute(item, args)
  }

  /** Runs a preset, or fills it into the draft when the preset is meant to be edited first. */
  async function pickPreset(item: SlashItem, preset: SlashPreset) {
    if (!preset.execute) {
      complete(item, preset)
      return
    }
    options.setDraft("")
    options.resize()
    await execute(item, preset.value.trim())
  }

  /** Returns true when the key was consumed by the menu. */
  function handleKey(event: KeyboardEvent) {
    if (event.isComposing || event.ctrlKey || event.altKey || event.metaKey) return false
    if (!open() && event.key !== "Enter" && event.key !== "Tab" && event.key !== "Escape") return false
    // Shift+Enter inserts a newline rather than accepting the highlighted entry.
    if (event.key === "Enter" && event.shiftKey) return false
    const item = argumentItem()
    const presets = argumentPresets()
    if (event.key === "Tab") {
      if (event.shiftKey) return false
      const command = item ?? matches()[activeMatchIndex()]
      if (!command) return false
      // Tab is completion only, including presets whose Enter action executes immediately.
      if (!item || presets.length) complete(command, item ? presets[activePresetIndex()] : undefined)
      event.preventDefault()
      return true
    }
    // When a command is fixed the menu shows its presets, but a command with no presets still
    // occupies one row so the cursor has something to sit on.
    const count = item ? Math.max(1, presets.length) : matches().length
    const atEnd = options.area().selectionStart === options.draft().length &&
      options.area().selectionEnd === options.draft().length

    if (event.key === "ArrowRight" && presets.length && atEnd) {
      setExpandedArgument(presets[activePresetIndex()].value)
    } else if (event.key === "ArrowLeft" && expandedArgument() && atEnd) setExpandedArgument(undefined)
    else if (event.key === "ArrowDown") setCursor(Math.min(cursor() + 1, count - 1))
    else if (event.key === "ArrowUp") setCursor(Math.max(cursor() - 1, 0))
    else if (event.key === "Escape") setDismissed(true)
    else if (event.key !== "Enter") return false
    else if (!item) {
      const match = matches()[activeMatchIndex()]
      if (!match) return false
      void pick(match)
    } else if (presets.length) void pickPreset(item, presets[activePresetIndex()])
    // With arguments already typed, or a command that needs none, accepting runs it directly.
    else if (parsed()?.args || !item.requiredArgs) void pick(item)
    else return false

    event.preventDefault()
    return true
  }

  /** Completed commands still accept Enter even after their suggestion popup closes. */
  const active = () => matches().length > 0
  const open = () => active() && (!argumentItem() || argumentPresets().length > 0 || !!argumentHelp().usage)
  const activeOptionId = () => {
    if (!open()) return undefined
    if (argumentItem()) return argumentPresets().length ? `${id}-arg-${activePresetIndex()}` : undefined
    return `${id}-command-${activeMatchIndex()}`
  }

  return {
    id,
    activeOptionId,
    parsed,
    matches,
    argumentItem,
    argumentPresets,
    argumentHelp,
    expandedArgument,
    toggleArgumentHelp,
    cursor,
    setCursor,
    activeMatchIndex,
    activePresetIndex,
    dismissed,
    setDismissed,
    active,
    open,
    pick,
    pickPreset,
    handleKey,
  }
}
