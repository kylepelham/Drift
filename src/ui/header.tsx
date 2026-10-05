import { createSignal, onCleanup, Show } from "solid-js"
import { useEngine } from "../engine"
import { selectedSession, selectSession } from "../state/selection"
import { openMobileDrawer } from "../state/navigation"
import { ContextMeter } from "./context-meter"
import { IconArrowUp, IconMenu, IconSearch } from "./icons"
import { openTranscriptFind, TranscriptFindBar, transcriptFindOpen } from "./transcript-find"
import { t } from "../state/i18n"

const chatColumnWidth = 768

export function ChatHeader() {
  const engine = useEngine()
  const session = () => engine.state.sessions[selectedSession() ?? ""]
  const [transparent, setTransparent] = createSignal(false)
  let row!: HTMLDivElement

  function remeasure() {
    const left = row.firstElementChild?.getBoundingClientRect().width ?? 0
    const right = row.lastElementChild?.getBoundingClientRect().width ?? 0
    const gap = Math.max(0, (row.clientWidth - chatColumnWidth) / 2)
    setTransparent(gap > left + 16 && gap > right + 16)
  }

  function observe(element: HTMLDivElement) {
    row = element
    const observer = new ResizeObserver(remeasure)
    observer.observe(element)
    for (const child of element.children) observer.observe(child)
    onCleanup(() => observer.disconnect())
  }

  const backTarget = () => {
    const current = session()
    if (!current) return undefined
    return current.parentID ?? engine.state.links[current.id]
  }
  return (
    <Show
      when={session()}
      fallback={
        <button
          class="mobile-menu-button absolute top-0 left-2 z-10 hidden size-11 items-center justify-center rounded-md text-ink-muted hover:bg-raised hover:text-ink"
          title={t("drift.mobile.openNavigation")}
          onClick={openMobileDrawer}
        >
          <IconMenu />
        </button>
      }
    >
      <div
        ref={observe}
        class="pointer-events-none absolute inset-x-0 top-0 z-10 flex h-11 items-center gap-2 border-b px-4 transition-colors"
        classList={{
          "border-edge bg-bg": !transparent(),
          "border-transparent bg-transparent": transparent(),
        }}
      >
        <div class="pointer-events-auto flex min-w-0 max-w-[60%] items-center gap-2">
          <button
            class="mobile-menu-button hidden size-11 shrink-0 items-center justify-center rounded-md text-ink-muted hover:bg-raised hover:text-ink"
            title={t("drift.mobile.openNavigation")}
            onClick={openMobileDrawer}
          >
            <IconMenu />
          </button>
          <Show when={session()}>
            {(current) => (
              <>
                <Show when={backTarget()}>
                  {(target) => (
                    <button
                      class="flex size-7 shrink-0 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
                      title={t("drift.thread.backToParent")}
                      onClick={() => selectSession(target())}
                    >
                      <IconArrowUp />
                    </button>
                  )}
                </Show>
                <Title id={current().id} title={current().title} />
              </>
            )}
          </Show>
        </div>
        <div class="pointer-events-none min-w-4 flex-1" />
        <Show when={session()}>
          {(current) => (
            <div class="pointer-events-auto flex shrink-0 items-center gap-2">
              <TranscriptFindBar />
              <Show when={!transcriptFindOpen()}>
                <button
                  class="flex size-7 shrink-0 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
                  title={t("drift.search.transcript")}
                  onClick={openTranscriptFind}
                >
                  <IconSearch class="size-3.5" />
                </button>
              </Show>
              <ContextMeter sessionId={current().id} />
            </div>
          )}
        </Show>
      </div>
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
          class="min-w-0 cursor-text truncate text-sm text-ink"
          title={t("drift.thread.renameHint")}
          onDblClick={() => setEditing(true)}
        >
          {props.title || t("drift.thread.untitled")}
        </span>
      }
    >
      <input
        class="w-64 min-w-0 rounded-md border border-edge bg-surface px-2 py-1 text-sm outline-none focus:border-edge-strong"
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
