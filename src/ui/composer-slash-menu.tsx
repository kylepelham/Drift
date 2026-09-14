import { createEffect, For, Show } from "solid-js"
import { t } from "../state/i18n"
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
    <div class="pop-in absolute bottom-full left-3 z-20 mb-2 w-96 max-w-[calc(100%_-_1.5rem)] overflow-hidden rounded-lg border border-edge bg-overlay shadow-xl shadow-black/30">
      <Show when={menu.argumentItem()}>
        {(item) => (
          <div class="border-b border-edge px-3 py-2">
            <div class="font-mono text-xs text-accent">/{item().name}</div>
            <Show when={menu.argumentHelp().usage}>
              <div class="mt-1 text-xs break-words text-ink-muted">{menu.argumentHelp().usage}</div>
            </Show>
          </div>
        )}
      </Show>
      <div ref={list} id={menu.id} role="listbox" class="max-h-[min(20rem,50vh)] overflow-y-auto overscroll-contain py-1">
        <Show when={menu.argumentItem()} fallback={
          <For each={menu.matches()}>
            {(item, index) => (
              <button
                type="button" role="option" tabIndex={-1}
                id={`${menu.id}-command-${index()}`}
                aria-selected={index() === menu.activeMatchIndex()}
                class="flex w-full flex-col gap-0.5 px-3 py-2 text-left transition-colors"
                classList={{ "bg-raised": index() === menu.activeMatchIndex() }}
                onMouseEnter={() => menu.setCursor(index())}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => void menu.pick(item)}
              >
                <span class="font-mono text-xs text-accent">/{item.name}</span>
                <span class="text-xs break-words text-ink-muted">{item.description}</span>
              </button>
            )}
          </For>
        }>
          {(item) => (
            <Show when={menu.argumentPresets().length} fallback={
              <div class="px-3 py-2 text-xs break-words text-ink-muted">{menu.argumentHelp().description}</div>
            }>
              <For each={menu.argumentPresets()}>
                {(preset, index) => (
                  <button
                    type="button" role="option" tabIndex={-1}
                    id={`${menu.id}-arg-${index()}`}
                    aria-selected={index() === menu.activePresetIndex()}
                    class="flex w-full flex-col gap-0.5 px-3 py-2 text-left transition-colors"
                    classList={{ "bg-raised": index() === menu.activePresetIndex() }}
                    onMouseEnter={() => menu.setCursor(index())}
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={() => void menu.pickPreset(item(), preset)}
                  >
                    <span class="font-mono text-xs break-words text-accent">{preset.label}{preset.usage ? ` ${preset.usage}` : ""}</span>
                    <Show when={preset.description}><span class="text-xs break-words text-ink-muted">{preset.description}</span></Show>
                  </button>
                )}
              </For>
            </Show>
          )}
        </Show>
      </div>
      <div class="border-t border-edge px-3 py-1.5 text-xs text-ink-muted">{t("drift.slash.keyboardHint")}</div>
    </div>
  )
}
