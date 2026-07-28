import { createEffect, createMemo, createSignal, For, Show, type JSX } from "solid-js"
import { Portal } from "solid-js/web"
import { createDismissOnOutside } from "./dismiss"
import { fixedMenuPosition } from "../state/zoom"
import { t } from "../state/i18n"
import { IconSliders } from "./icons"

export type PickerItem = {
  id: string
  label: string
  hint?: string
  detail?: string
  group?: string
  providerID?: string
  family?: string
  releaseDate?: string
}

export function Picker(props: {
  items: PickerItem[]
  selected?: string
  label: string
  icon?: JSX.Element
  fallbackLabel?: string
  onManage?: () => void
  floating?: boolean
  bordered?: boolean
  chevronAtEnd?: boolean
  placement?: "above" | "below"
  width?: string
  onPick: (id: string) => void
}) {
  const [open, setOpen] = createSignal(false)
  const [query, setQuery] = createSignal("")
  const [cursor, setCursor] = createSignal(0)
  let root!: HTMLDivElement
  let input!: HTMLInputElement
  let panel!: HTMLDivElement
  const [position, setPosition] = createSignal({ left: 0, top: 0 })

  const filtered = createMemo(() => {
    const needle = query().toLowerCase()
    return props.items.filter(
      (item) =>
        item.label.toLowerCase().includes(needle) ||
        item.id.toLowerCase().includes(needle) ||
        item.detail?.toLowerCase().includes(needle) ||
        item.group?.toLowerCase().includes(needle),
    )
  })

  createEffect(() => {
    if (!open()) return
    setQuery("")
    setCursor(0)
    if (props.floating) {
      const rect = root.getBoundingClientRect()
      const width = 288
      const height = 336
      const y = props.placement === "below" ? rect.bottom + 8 : rect.top - height - 8
      const next = fixedMenuPosition(rect.left, y, width, height)
      setPosition({ left: next.left, top: next.top })
    }
    queueMicrotask(() => input?.focus())
  })

  createDismissOnOutside({
    // A floating picker renders its panel in a portal, so the panel is outside `root` in the DOM
    // but still counts as inside for dismissal. Only a floating panel is position-dependent, so
    // only it closes on resize.
    enabled: open,
    inside: () => [root, panel],
    onDismiss: () => setOpen(false),
    resize: props.floating,
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

  const menu = () => (
    <div
      ref={panel}
      data-wheel-lock
      class="pop-in z-50 w-72 overflow-hidden rounded-lg border border-edge bg-overlay shadow-xl shadow-black/30"
      classList={{ "fixed": !!props.floating, "absolute bottom-full left-0 mb-2": !props.floating }}
      style={props.floating ? { left: `${position().left}px`, top: `${position().top}px` } : undefined}
    >
      <div class="flex items-center border-b border-edge">
        <input
          ref={input}
          class="min-w-0 flex-1 bg-transparent px-3 py-2 text-sm outline-none placeholder:text-ink-faint"
          placeholder={t("common.search.placeholder")}
          value={query()}
          onInput={(event) => {
            setQuery(event.currentTarget.value)
            setCursor(0)
          }}
          onKeyDown={onKey}
        />
        <Show when={props.onManage}>
          <button
            title={t("dialog.model.manage")}
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
                <span class="min-w-0 flex-1">
                  <span class="block truncate">{item.label}</span>
                  <Show when={item.detail}>
                    <span class="block truncate text-[0.68rem] text-ink-faint">{item.detail}</span>
                  </Show>
                </span>
              </button>
            </>
          )}
        </For>
        <Show when={filtered().length === 0}>
          <div class="px-3 py-2 text-sm text-ink-faint">{t("prompt.popover.emptyResults")}</div>
        </Show>
      </div>
    </div>
  )

  return (
    <div ref={root} class="relative">
      <button
        class="flex items-center gap-1.5 rounded-md px-2 py-1 text-xs font-medium text-ink-muted transition-colors hover:bg-raised hover:text-ink"
        classList={{ "border border-edge bg-overlay hover:border-edge-strong": !!props.bordered }}
        style={{ width: props.width }}
        title={props.label}
        onClick={() => setOpen(!open())}
      >
        {props.icon}
        <span class="min-w-0 max-w-40 truncate" classList={{ "flex-1": !!props.chevronAtEnd }}>
          {props.items.find((item) => item.id === props.selected)?.label ?? props.fallbackLabel ?? t("common.default")}
        </span>
        <svg
          class="size-2.5 shrink-0 text-ink-faint"
          style={{ "margin-inline-start": props.chevronAtEnd ? "auto" : undefined }}
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
        >
          <path d="M4 6l4 4 4-4" />
        </svg>
      </button>
      <Show when={open()}>
        <Show when={props.floating} fallback={menu()}>
          <Portal>{menu()}</Portal>
        </Show>
      </Show>
    </div>
  )
}
