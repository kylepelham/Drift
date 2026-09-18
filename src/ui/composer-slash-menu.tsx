import { createEffect, For, Show } from "solid-js"
import { t } from "../state/i18n"
import { Chevron } from "./controls"
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
              <div class="truncate px-3 py-2 text-xs text-ink-faint" title={menu.argumentHelp().usage}>{menu.argumentHelp().usage ?? `/${item().name}`}</div>
            }>
              <For each={menu.argumentPresets()}>
                {(preset, index) => (
                  <div classList={{ "bg-raised": index() === menu.activePresetIndex() }}>
                    <div class="flex items-center pr-1">
                      <button
                        type="button" role="option" tabIndex={-1}
                        id={`${menu.id}-arg-${index()}`}
                        aria-selected={index() === menu.activePresetIndex()}
                        aria-describedby={menu.expandedArgument() === preset.value ? `${menu.id}-details-${index()}` : undefined}
                        class="flex min-w-0 flex-1 items-baseline gap-2.5 py-1.5 pr-1 pl-3 text-left transition-colors"
                        onMouseEnter={() => menu.setCursor(index())}
                        onMouseDown={(event) => event.preventDefault()}
                        onClick={() => void menu.pickPreset(item(), preset)}
                      >
                        <span class="max-w-[45%] shrink-0 truncate font-mono text-xs text-accent" title={[preset.label, preset.usage].filter(Boolean).join(" ")}>{preset.label}</span>
                        <span class="min-w-0 flex-1 truncate text-xs text-ink-faint" title={preset.description}>{preset.description}</span>
                      </button>
                      <button
                        type="button" tabIndex={-1}
                        class="flex size-6 shrink-0 items-center justify-center rounded text-ink-faint hover:bg-overlay hover:text-ink"
                        aria-label={t("drift.slash.argumentDetails", { name: preset.label })}
                        title={t("drift.slash.argumentDetails", { name: preset.label })}
                        aria-expanded={menu.expandedArgument() === preset.value}
                        aria-controls={`${menu.id}-details-${index()}`}
                        onMouseDown={(event) => event.preventDefault()}
                        onClick={() => {
                          menu.setCursor(index())
                          menu.toggleArgumentHelp(preset)
                        }}
                      >
                        <Chevron open={menu.expandedArgument() === preset.value} />
                      </button>
                    </div>
                    <Show when={menu.expandedArgument() === preset.value}>
                      <div id={`${menu.id}-details-${index()}`} class="px-3 pt-1 pb-2 text-xs break-words text-ink-muted">
                        <div class="mb-1 font-mono text-accent">{[preset.label, preset.usage].filter(Boolean).join(" ")}</div>
                        <div>{preset.description}</div>
                      </div>
                    </Show>
                  </div>
                )}
              </For>
            </Show>
          )}
        </Show>
      </div>
    </div>
  )
}
