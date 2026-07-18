import { Show } from "solid-js"
import { useEngine } from "../engine"
import { selectedSession, selectSession } from "../state/selection"
import { IconArrowUp } from "./icons"

export function ChatHeader() {
  const engine = useEngine()
  const session = () => engine.state.sessions[selectedSession() ?? ""]
  return (
    <Show when={session()}>
      {(current) => (
        <div class="flex h-11 shrink-0 items-center gap-2 border-b border-edge px-4">
          <Show when={current().parentID}>
            {(parentId) => (
              <button
                class="flex size-7 shrink-0 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
                title="Back to parent thread"
                onClick={() => selectSession(parentId())}
              >
                <IconArrowUp />
              </button>
            )}
          </Show>
          <span class="min-w-0 flex-1 truncate text-sm text-ink">{current().title || "Untitled"}</span>
        </div>
      )}
    </Show>
  )
}
