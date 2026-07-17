import type { Session } from "@opencode-ai/sdk/client"
import { createMemo, For, Show } from "solid-js"
import { useEngine } from "../engine"
import { sessionBusy, visibleSessions } from "../engine/store"
import { selectedSession, selectSession } from "../state/selection"
import { cycleTheme, theme } from "../state/theme"

export function Sidebar() {
  const engine = useEngine()
  const sessions = createMemo(() => visibleSessions(engine.state))

  return (
    <aside class="flex w-64 shrink-0 flex-col border-r border-edge bg-surface">
      <div class="flex items-center justify-between px-4 pt-4 pb-3">
        <span class="text-sm font-semibold tracking-wide text-ink">drift</span>
        <button
          class="rounded-md border border-edge px-2 py-1 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink"
          onClick={() => selectSession(null)}
        >
          + New
        </button>
      </div>
      <nav class="min-h-0 flex-1 space-y-0.5 overflow-y-auto px-2">
        <For each={sessions()}>{(session) => <ThreadItem session={session} />}</For>
        <Show when={sessions().length === 0}>
          <div class="px-2 py-4 text-xs text-ink-faint">No threads yet</div>
        </Show>
      </nav>
      <SidebarFooter />
    </aside>
  )
}

function ThreadItem(props: { session: Session }) {
  const engine = useEngine()
  const active = () => selectedSession() === props.session.id
  return (
    <div
      class="group flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 transition-colors"
      classList={{ "bg-raised": active(), "hover:bg-raised/60": !active() }}
      onClick={() => selectSession(props.session.id)}
    >
      <Show when={sessionBusy(engine.state, props.session.id)}>
        <span class="pulse-soft size-1.5 shrink-0 rounded-full bg-accent" />
      </Show>
      <div class="min-w-0 flex-1">
        <div class="truncate text-sm" classList={{ "text-ink": active(), "text-ink-muted": !active() }}>
          {props.session.title || "Untitled"}
        </div>
        <div class="text-[0.68rem] text-ink-faint">{ago(props.session.time.updated)}</div>
      </div>
      <button
        class="hidden shrink-0 text-ink-faint transition-colors hover:text-danger group-hover:block"
        title="Delete thread"
        onClick={(event) => {
          event.stopPropagation()
          if (selectedSession() === props.session.id) selectSession(null)
          void engine.actions.remove(props.session.id)
        }}
      >
        <svg class="size-3.5" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
          <path d="M4 4l8 8M12 4l-8 8" />
        </svg>
      </button>
    </div>
  )
}

function SidebarFooter() {
  const engine = useEngine()
  const dot: Record<string, string> = { online: "bg-ok", connecting: "bg-warn pulse-soft", offline: "bg-danger" }
  return (
    <div class="flex items-center gap-2 border-t border-edge px-4 py-2.5 text-xs text-ink-faint">
      <span class={`size-1.5 rounded-full ${dot[engine.state.connection]}`} />
      <span class="min-w-0 flex-1 truncate" title={engine.state.directory}>
        {engine.state.connection === "online" ? shortPath(engine.state.directory) : engine.state.connection}
      </span>
      <button class="transition-colors hover:text-ink" title={`Theme: ${theme()}`} onClick={cycleTheme}>
        <svg class="size-3.5" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
          <circle cx="8" cy="8" r="6" />
          <path d="M8 2a6 6 0 000 12z" fill="currentColor" stroke="none" />
        </svg>
      </button>
    </div>
  )
}

function shortPath(path: string) {
  const parts = path.replaceAll("\\", "/").split("/").filter(Boolean)
  return parts.slice(-2).join("/") || path
}

function ago(timestamp: number) {
  const seconds = Math.max(0, (Date.now() - timestamp) / 1000)
  if (seconds < 60) return "just now"
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`
  return `${Math.floor(seconds / 86400)}d ago`
}
