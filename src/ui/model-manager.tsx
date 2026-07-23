import { createEffect, createMemo, createSignal, For, onCleanup, Show } from "solid-js"
import { modelVisible, moveModelProvider, setModelsVisible, setModelVisible } from "../state/prefs"
import { IconX } from "./icons"
import { dragReorder } from "./drag-reorder"
import { Chevron } from "./parts"
import type { PickerItem } from "./picker"

export function ModelManager(props: { items: PickerItem[]; onClose: () => void }) {
  const [query, setQuery] = createSignal("")
  const [expanded, setExpanded] = createSignal<string[]>([])
  const defaults = createMemo(() => defaultVisibleModelIds(props.items))
  const filtered = createMemo(() => {
    const value = query().toLowerCase()
    return props.items.filter(
      (item) =>
        item.label.toLowerCase().includes(value) ||
        item.id.toLowerCase().includes(value) ||
        item.group?.toLowerCase().includes(value),
    )
  })
  const providers = createMemo(() => providerGroups(props.items))
  const groups = createMemo(() => providerGroups(filtered()))
  const providerIDs = () => providers().map((provider) => provider.id)
  const enabled = (item: PickerItem) => modelVisible(item.id, defaults().has(item.id))
  const providerItems = (providerID: string, source = filtered()) =>
    sortManagerModelItems(
      source.filter((item) => (item.providerID ?? item.group ?? "Other") === providerID),
      enabled,
    )
  const providerOpen = (providerID: string) => !!query().trim() || expanded().includes(providerID)
  const toggleProvider = (providerID: string) =>
    setExpanded((current) =>
      current.includes(providerID) ? current.filter((id) => id !== providerID) : [...current, providerID],
    )
  createEffect(() => {
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") props.onClose()
    }
    document.addEventListener("keydown", escape)
    onCleanup(() => document.removeEventListener("keydown", escape))
  })

  return (
    <div data-wheel-lock class="fixed inset-0 z-30 flex items-center justify-center bg-black/50" onClick={props.onClose}>
      <div
        class="fade-up flex max-h-[80vh] w-[35rem] flex-col overflow-hidden rounded-2xl border border-edge bg-overlay shadow-2xl shadow-black/40"
        onClick={(event) => event.stopPropagation()}
      >
        <div class="flex items-center justify-between border-b border-edge px-5 py-4">
          <div class="text-sm font-semibold tracking-wide text-ink">Manage models</div>
          <button
            title="Close"
            class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
            onClick={props.onClose}
          >
            <IconX />
          </button>
        </div>
        <div class="border-b border-edge px-4 py-3">
          <input
            autofocus
            class="w-full rounded-xl border border-edge bg-surface px-3.5 py-2.5 text-sm outline-none transition-colors placeholder:text-ink-faint focus:border-edge-strong"
            placeholder="Search models..."
            value={query()}
            onInput={(event) => setQuery(event.currentTarget.value)}
          />
        </div>
        <div class="min-h-0 flex-1 overflow-y-auto p-3.5">
          <For each={groups()}>
            {(provider) => {
              let root!: HTMLElement
              const all = () => providerItems(provider.id, props.items)
              const visibleItems = () => providerItems(provider.id)
              const enabledCount = () => all().filter(enabled).length
              const allEnabled = () => all().length > 0 && enabledCount() === all().length
              return (
                <section
                  ref={root}
                  data-provider={provider.id}
                  class="mb-2 overflow-hidden rounded-xl border border-edge bg-surface/20 last:mb-0"
                >
                  <div
                    class="flex min-h-12 cursor-pointer items-center gap-2 px-3 transition-colors select-none hover:bg-raised/50"
                    aria-expanded={providerOpen(provider.id)}
                    onPointerDown={(event) =>
                      dragReorder(event, root, {
                        selector: ":scope > [data-provider]",
                        id: provider.id,
                        itemID: (element) => element.dataset.provider ?? "",
                        move: (id, beforeID) => moveModelProvider(id, beforeID, providerIDs()),
                        dragged: markProviderDragged,
                      })
                    }
                    onClick={() => {
                      if (!providerDragged) toggleProvider(provider.id)
                    }}
                  >
                    <span class="flex size-5 shrink-0 items-center justify-center text-ink-faint">
                      <Chevron open={providerOpen(provider.id)} />
                    </span>
                    <span class="min-w-0 flex-1 truncate text-sm font-medium text-ink">{provider.label}</span>
                    <span class="shrink-0 rounded-full bg-raised px-2 py-0.5 text-[0.65rem] tabular-nums text-ink-faint">
                      {enabledCount()} of {all().length}
                    </span>
                    <Toggle
                      label={`${allEnabled() ? "Hide" : "Show"} all ${provider.label} models`}
                      checked={allEnabled()}
                      onChange={() => setModelsVisible(all().map((item) => item.id), !allEnabled())}
                    />
                  </div>
                  <Show when={providerOpen(provider.id)}>
                    <div class="border-t border-edge bg-overlay/30 p-1.5">
                      <For each={visibleItems()}>
                        {(item) => (
                          <button
                            role="switch"
                            aria-checked={enabled(item)}
                            class="flex w-full cursor-pointer items-center justify-between rounded-lg px-2 py-2 text-left hover:bg-raised/60"
                            onClick={() => setModelVisible(item.id, !enabled(item))}
                          >
                            <span class="min-w-0 truncate text-sm text-ink">{item.label}</span>
                            <ToggleTrack checked={enabled(item)} />
                          </button>
                        )}
                      </For>
                    </div>
                  </Show>
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

function providerGroups(items: PickerItem[]) {
  const groups = new Map<string, string>()
  for (const item of items) {
    const id = item.providerID ?? item.group ?? "Other"
    if (!groups.has(id)) groups.set(id, item.group ?? "Other")
  }
  return [...groups].map(([id, label]) => ({ id, label }))
}

export function sortManagerModelItems(items: PickerItem[], visible: (item: PickerItem) => boolean) {
  return [...items].sort((a, b) => Number(visible(b)) - Number(visible(a)) || a.label.localeCompare(b.label))
}

let providerDragged = false

function markProviderDragged() {
  providerDragged = true
  setTimeout(() => (providerDragged = false), 0)
}

const sixMonths = 365.25 * 24 * 60 * 60 * 1000 / 2

export function defaultVisibleModelIds(items: PickerItem[], now = Date.now()) {
  const visible = new Set<string>()
  const latest = new Map<string, { id: string; released: number }>()
  for (const item of items) {
    const released = Date.parse(item.releaseDate ?? "")
    if (!Number.isFinite(released)) {
      visible.add(item.id)
      continue
    }
    if (Math.abs(released - now) >= sixMonths) continue
    const family = `${item.providerID ?? item.group ?? ""}:${item.family ?? ""}`
    const current = latest.get(family)
    if (!current || released > current.released) latest.set(family, { id: item.id, released })
  }
  for (const item of latest.values()) visible.add(item.id)
  return visible
}

export function Toggle(props: { label: string; checked: boolean; disabled?: boolean; onChange: () => void }) {
  return (
    <button
      role="switch"
      aria-label={props.label}
      aria-checked={props.checked}
      disabled={props.disabled}
      class="shrink-0 disabled:opacity-50"
      onPointerDown={(event) => event.stopPropagation()}
      onClick={(event) => {
        event.stopPropagation()
        props.onChange()
      }}
    >
      <ToggleTrack checked={props.checked} />
    </button>
  )
}

function ToggleTrack(props: { checked: boolean }) {
  return (
    <span
      class="relative block h-4 w-7 shrink-0 rounded-full border transition-colors"
      classList={{
        "border-accent bg-accent": props.checked,
        "border-edge-strong bg-raised": !props.checked,
      }}
    >
      <span
        class="absolute top-0.5 left-0.5 size-2.5 rounded-full bg-white transition-transform"
        classList={{ "translate-x-3": props.checked }}
      />
    </span>
  )
}
