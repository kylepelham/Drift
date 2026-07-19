import { createEffect, createMemo, createSignal, For, onCleanup, Show } from "solid-js"
import { hiddenModelIds, setModelVisible } from "../state/prefs"
import { IconX } from "./icons"
import type { PickerItem } from "./picker"

export function ModelManager(props: { items: PickerItem[]; onClose: () => void }) {
  const [query, setQuery] = createSignal("")
  const filtered = createMemo(() => {
    const value = query().toLowerCase()
    return props.items.filter(
      (item) =>
        item.label.toLowerCase().includes(value) ||
        item.id.toLowerCase().includes(value) ||
        item.group?.toLowerCase().includes(value),
    )
  })
  const groups = createMemo(() => [...new Set(filtered().map((item) => item.group ?? "Other"))])

  createEffect(() => {
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") props.onClose()
    }
    document.addEventListener("keydown", escape)
    onCleanup(() => document.removeEventListener("keydown", escape))
  })

  return (
    <div class="fixed inset-0 z-30 flex items-center justify-center bg-black/50" onClick={props.onClose}>
      <div
        class="fade-up flex max-h-[78vh] w-[34rem] flex-col overflow-hidden rounded-xl border border-edge bg-overlay shadow-2xl shadow-black/40"
        onClick={(event) => event.stopPropagation()}
      >
        <div class="flex items-start justify-between border-b border-edge px-4 py-3">
          <div>
            <div class="text-sm font-semibold text-ink">Manage models</div>
            <div class="mt-0.5 text-xs text-ink-faint">Choose which models appear in the model picker.</div>
          </div>
          <button
            title="Close"
            class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
            onClick={props.onClose}
          >
            <IconX />
          </button>
        </div>
        <div class="border-b border-edge p-3">
          <input
            autofocus
            class="w-full rounded-lg border border-edge bg-surface px-3 py-2 text-sm outline-none placeholder:text-ink-faint focus:border-edge-strong"
            placeholder="Search models..."
            value={query()}
            onInput={(event) => setQuery(event.currentTarget.value)}
          />
        </div>
        <div class="min-h-0 flex-1 overflow-y-auto p-3">
          <For each={groups()}>
            {(group) => {
              const visibleItems = () => filtered().filter((item) => (item.group ?? "Other") === group)
              return (
                <section class="mb-4 last:mb-0">
                  <div class="px-2 py-1.5 text-xs font-medium text-ink-faint">{group}</div>
                  <For each={visibleItems()}>
                    {(item) => {
                      const enabled = () => !hiddenModelIds().includes(item.id)
                      return (
                        <div class="flex items-center justify-between rounded-lg px-2 py-2 hover:bg-raised/60">
                          <span class="min-w-0 truncate text-sm text-ink">{item.label}</span>
                          <Toggle label={`Show ${item.label}`} checked={enabled()} onChange={() => setModelVisible(item.id, !enabled())} />
                        </div>
                      )
                    }}
                  </For>
                </section>
              )
            }}
          </For>
          <Show when={filtered().length === 0}>
            <div class="py-8 text-center text-sm text-ink-faint">No matching models.</div>
          </Show>
        </div>
      </div>
    </div>
  )
}

export function Toggle(props: { label: string; checked: boolean; onChange: () => void }) {
  return (
    <button
      role="switch"
      aria-label={props.label}
      aria-checked={props.checked}
      class="relative h-4 w-7 shrink-0 rounded-full border transition-colors"
      classList={{
        "border-accent bg-accent": props.checked,
        "border-edge-strong bg-raised": !props.checked,
      }}
      onClick={(event) => {
        event.stopPropagation()
        props.onChange()
      }}
    >
      <span
        class="absolute top-0.5 left-0.5 size-2.5 rounded-full bg-white transition-transform"
        classList={{ "translate-x-3": props.checked }}
      />
    </button>
  )
}
