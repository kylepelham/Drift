import { createSignal, Show } from "solid-js"
import { useEngine } from "../engine"
import { selectedSession, selectSession } from "../state/selection"
import { IconArrowUp } from "./icons"

export function ChatHeader() {
  const engine = useEngine()
  const session = () => engine.state.sessions[selectedSession() ?? ""]
  const backTarget = () => {
    const current = session()
    if (!current) return undefined
    return current.parentID ?? engine.state.links[current.id]
  }
  return (
    <Show when={session()}>
      {(current) => (
        <div class="flex h-11 shrink-0 items-center gap-2 border-b border-edge px-4">
          <Show when={backTarget()}>
            {(target) => (
              <button
                class="flex size-7 shrink-0 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
                title="Back to the thread this was spawned from"
                onClick={() => selectSession(target())}
              >
                <IconArrowUp />
              </button>
            )}
          </Show>
          <Title id={current().id} title={current().title} />
        </div>
      )}
    </Show>
  )
}

function Title(props: { id: string; title: string }) {
  const engine = useEngine()
  const [editing, setEditing] = createSignal(false)

  const commit = (value: string) => {
    const next = value.trim()
    if (next && next !== props.title) void engine.actions.rename(props.id, next)
    setEditing(false)
  }

  return (
    <Show
      when={editing()}
      fallback={
        <span
          class="min-w-0 flex-1 cursor-text truncate text-sm text-ink"
          title="Double-click to rename"
          onDblClick={() => setEditing(true)}
        >
          {props.title || "Untitled"}
        </span>
      }
    >
      <input
        class="min-w-0 flex-1 rounded-md border border-edge bg-surface px-2 py-1 text-sm outline-none focus:border-edge-strong"
        value={props.title}
        ref={(el) => queueMicrotask(() => el.select())}
        onKeyDown={(event) => {
          if (event.key === "Enter") commit(event.currentTarget.value)
          if (event.key === "Escape") setEditing(false)
        }}
        onBlur={(event) => commit(event.currentTarget.value)}
      />
    </Show>
  )
}
