import { createEffect, createMemo, createSignal, For, onCleanup, Show, type JSX } from "solid-js"
import { IconSliders } from "./icons"

export type PickerItem = { id: string; label: string; hint?: string; group?: string }

export function Picker(props: {
  items: PickerItem[]
  selected?: string
  label: string
  icon?: JSX.Element
  fallbackLabel?: string
  onManage?: () => void
  onPick: (id: string) => void
}) {
  const [open, setOpen] = createSignal(false)
  const [query, setQuery] = createSignal("")
  const [cursor, setCursor] = createSignal(0)
  let root!: HTMLDivElement
  let input!: HTMLInputElement

  const filtered = createMemo(() => {
    const q = query().toLowerCase()
    return props.items.filter(
      (item) =>
        item.label.toLowerCase().includes(q) ||
        item.id.toLowerCase().includes(q) ||
        item.group?.toLowerCase().includes(q),
    )
  })

  createEffect(() => {
    if (!open()) return
    setQuery("")
    setCursor(0)
    queueMicrotask(() => input?.focus())
    const away = (event: MouseEvent) => {
      if (!root.contains(event.target as Node)) setOpen(false)
    }
    document.addEventListener("mousedown", away)
    onCleanup(() => document.removeEventListener("mousedown", away))
  })

  const pick = (id: string) => {
    props.onPick(id)
    setOpen(false)
  }

  const onKey = (event: KeyboardEvent) => {
    if (event.key === "Escape") setOpen(false)
    if (event.key === "ArrowDown") setCursor(Math.min(cursor() + 1, filtered().length - 1))
    if (event.key === "ArrowUp") setCursor(Math.max(cursor() - 1, 0))
    if (event.key === "Enter" && filtered()[cursor()]) pick(filtered()[cursor()].id)
    if (["Escape", "ArrowDown", "ArrowUp", "Enter"].includes(event.key)) event.preventDefault()
  }

  return (
    <div ref={root} class="relative">
      <button
        class="flex items-center gap-1.5 rounded-md px-2 py-1 text-xs font-medium text-ink-muted transition-colors hover:bg-raised hover:text-ink"
        title={props.label}
        onClick={() => setOpen(!open())}
      >
        {props.icon}
        <span class="max-w-40 truncate">
          {props.items.find((item) => item.id === props.selected)?.label ?? props.fallbackLabel ?? "auto"}
        </span>
        <svg class="size-2.5 text-ink-faint" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M4 6l4 4 4-4" />
        </svg>
      </button>
      <Show when={open()}>
        <div class="pop-in absolute bottom-full left-0 z-20 mb-2 w-72 overflow-hidden rounded-lg border border-edge bg-overlay shadow-xl shadow-black/30">
          <div class="flex items-center border-b border-edge">
            <input
              ref={input}
              class="min-w-0 flex-1 bg-transparent px-3 py-2 text-sm outline-none placeholder:text-ink-faint"
              placeholder="Search..."
              value={query()}
              onInput={(event) => {
                setQuery(event.currentTarget.value)
                setCursor(0)
              }}
              onKeyDown={onKey}
            />
            <Show when={props.onManage}>
              <button
                title="Manage models"
                class="mr-1 flex size-7 shrink-0 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
                onClick={() => {
                  setOpen(false)
                  props.onManage?.()
                }}
              >
                <IconSliders class="size-4" />
              </button>
            </Show>
          </div>
          <div class="max-h-72 overflow-y-auto py-1">
            <For each={filtered()}>
              {(item, index) => (
                <>
                  <Show when={item.group && item.group !== filtered()[index() - 1]?.group}>
                    <div class="px-3 pt-3 pb-1 text-xs font-medium text-ink-faint first:pt-1.5">{item.group}</div>
                  </Show>
                  <button
                    class="flex w-full items-center px-3 py-1.5 text-left text-sm transition-colors"
                    classList={{
                      "bg-raised text-ink": index() === cursor(),
                      "text-ink-muted": index() !== cursor(),
                      "text-accent": item.id === props.selected,
                    }}
                    title={item.hint}
                    onMouseEnter={() => setCursor(index())}
                    onClick={() => pick(item.id)}
                  >
                    <span class="truncate">{item.label}</span>
                  </button>
                </>
              )}
            </For>
            <Show when={filtered().length === 0}>
              <div class="px-3 py-2 text-sm text-ink-faint">No matches</div>
            </Show>
          </div>
        </div>
      </Show>
    </div>
  )
}
