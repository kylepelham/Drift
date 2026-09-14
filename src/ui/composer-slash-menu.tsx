import { createEffect, For, Show } from "solid-js"
import type { createSlashMenu } from "./composer-slash"

export function ComposerSlashMenu(props: { menu: ReturnType<typeof createSlashMenu> }) {
  const menu = props.menu
  let list!: HTMLDivElement
  createEffect(() => {
    const id = menu.activeOptionId()
    queueMicrotask(() => {
      if (!id || !list?.isConnected) return
      const active = document.getElementById(id)
      if (active && list.contains(active)) active.scrollIntoView({ block: "nearest" })
    })
  })
  return (
    <div class="pop-in absolute bottom-full left-3 z-20 mb-2 w-80 max-w-[calc(100%_-_1.5rem)] overflow-hidden rounded-lg border border-edge bg-overlay shadow-xl shadow-black/30">
      <div ref={list} id={menu.id} role="listbox" class="max-h-[min(20rem,50vh)] overflow-y-auto overscroll-contain py-1">
        <Show when={menu.argumentItem()} fallback={
          <For each={menu.matches()}>
            {(item, index) => (
              <button
                type="button" role="option" tabIndex={-1}
                id={`${menu.id}-command-${index()}`}
                aria-selected={index() === menu.activeMatchIndex()}
                class="flex w-full items-baseline gap-2.5 px-3 py-1.5 text-left text-sm transition-colors"
                classList={{ "bg-raised": index() === menu.activeMatchIndex() }}
                onMouseEnter={() => menu.setCursor(index())}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => void menu.pick(item)}
              >
                <span class="shrink-0 font-mono text-xs text-accent">/{item.name}</span>
                <span class="min-w-0 truncate text-xs text-ink-faint" title={item.description}>{item.description}</span>
                <Show when={!item.engine && item.usage}>
                  <span class="ml-auto shrink-0 font-mono text-[0.65rem] text-ink-faint">{item.usage}</span>
                </Show>
              </button>
            )}
          </For>
        }>
          {(item) => (
            <Show when={menu.argumentPresets().length} fallback={
              <div class="px-3 py-2 text-xs text-ink-faint">{menu.argumentHelp().usage ?? `/${item().name}`}</div>
            }>
              <For each={menu.argumentPresets()}>
                {(preset, index) => (
                  <button
                    type="button" role="option" tabIndex={-1}
                    id={`${menu.id}-arg-${index()}`}
                    aria-selected={index() === menu.activePresetIndex()}
                    class="flex w-full items-start gap-2.5 px-3 py-1.5 text-left transition-colors"
                    classList={{ "bg-raised": index() === menu.activePresetIndex() }}
                    onMouseEnter={() => menu.setCursor(index())}
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={() => void menu.pickPreset(item(), preset)}
                  >
                    <span class="shrink-0 font-mono text-xs text-accent" title={preset.usage}>{preset.label}</span>
                    <span class="min-w-0 text-xs text-ink-faint">{preset.description}</span>
                  </button>
                )}
              </For>
            </Show>
          )}
        </Show>
      </div>
    </div>
  )
}
