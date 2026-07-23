import type { ToolPart } from "@opencode-ai/sdk/client"
import { createEffect, createSignal, For, onCleanup, Show } from "solid-js"
import { toolContextActions, type ToolContextAction } from "../tool-actions"
import { fixedMenuPosition } from "../state/zoom"

type MenuState = { x: number; y: number; actions: ToolContextAction[] }

const [menu, setMenu] = createSignal<MenuState | null>(null)

export function openToolContextMenu(event: MouseEvent, part: ToolPart) {
  const actions = toolContextActions(part)
  if (!actions.length) return
  event.preventDefault()
  event.stopPropagation()
  setMenu({ x: event.clientX, y: event.clientY, actions })
}

export function ToolContextMenuHost() {
  let root!: HTMLDivElement
  const position = () => {
    const state = menu()
    if (!state) return { left: 8, top: 8, viewportHeight: window.innerHeight }
    const estimatedHeight = state.actions.length * 38 + 20
    const viewport = fixedMenuPosition(state.x, state.y, 288, 0)
    return fixedMenuPosition(state.x, state.y, 288, Math.min(viewport.viewportHeight * 0.7, estimatedHeight))
  }

  createEffect(() => {
    if (!menu()) return
    const away = (event: MouseEvent) => {
      if (!root.contains(event.target as Node)) setMenu(null)
    }
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") setMenu(null)
    }
    const close = () => setMenu(null)
    document.addEventListener("mousedown", away)
    document.addEventListener("keydown", key)
    document.addEventListener("scroll", close, true)
    window.addEventListener("blur", close)
    window.addEventListener("resize", close)
    onCleanup(() => {
      document.removeEventListener("mousedown", away)
      document.removeEventListener("keydown", key)
      document.removeEventListener("scroll", close, true)
      window.removeEventListener("blur", close)
      window.removeEventListener("resize", close)
    })
  })

  return (
    <Show when={menu()}>
      {(state) => (
        <div
          ref={root}
          class="pop-in fixed z-50 max-h-[70vh] w-72 overflow-y-auto rounded-lg border border-edge bg-overlay p-1.5 shadow-xl shadow-black/40"
          style={{
            left: `${position().left}px`,
            top: `${position().top}px`,
            "max-height": `${position().viewportHeight * 0.7}px`,
          }}
          role="menu"
        >
          <For each={state().actions}>
            {(action) => (
              <button
                class="flex w-full items-center gap-3 rounded-md px-2.5 py-1.5 text-left text-sm text-ink-muted transition-colors hover:bg-raised hover:text-ink disabled:opacity-40"
                classList={{ "mt-1 border-t border-edge pt-2": !!action.separator }}
                disabled={action.disabled}
                role="menuitem"
                onClick={() => {
                  setMenu(null)
                  void Promise.resolve(action.run()).catch((error) =>
                    console.warn(`[Drift] Tool context action ${action.id} failed`, error),
                  )
                }}
              >
                <span class="min-w-0 flex-1 truncate">{action.label}</span>
                <Show when={action.detail}>{(detail) => <span class="shrink-0 text-xs text-ink-faint">{detail()}</span>}</Show>
              </button>
            )}
          </For>
        </div>
      )}
    </Show>
  )
}
