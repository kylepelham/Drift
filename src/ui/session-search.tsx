import { createEffect, For, onCleanup, Show } from "solid-js"
import { createStore } from "solid-js/store"
import { createSignal } from "solid-js"
import { useEngine } from "../engine"
import { sessionsFor } from "../engine/store"
import { t } from "../state/i18n"
import { selectSession } from "../state/selection"
import {
  createSessionSearchRunner,
  highlightSegments,
  sessionSearchDebounceMs,
  sessionSearchReady,
  type SessionSearchHit,
  type SessionSearchMode,
  type SessionSearchState,
} from "../state/session-search"
import { driftStore } from "../state/store"
import { archivedIds, selectWorkspace, workspaces } from "../state/workspaces"
import { IconSearch, IconX } from "./icons"

// One sidebar, one search: the query has to be readable by the input and by the list that replaces
// the workspace tree, so it lives beside them rather than inside either one.
const [query, setQuery] = createSignal("")
const [mode, setMode] = createSignal<SessionSearchMode>("name")
const [results, setResults] = createStore<SessionSearchState>({
  query: "",
  mode: "name",
  hits: [],
  loading: false,
  error: "",
})

/** Set when a content result is opened, so the transcript can scroll to the message that matched. */
const [pendingReveal, setPendingReveal] = createSignal<{ sessionId: string; messageId: string } | null>(null)

export function sessionSearchActive() {
  return sessionSearchReady(query())
}

export function revealTarget(sessionId: string) {
  const pending = pendingReveal()
  return pending?.sessionId === sessionId ? pending.messageId : undefined
}

export function clearReveal() {
  setPendingReveal(null)
}

function updatedLabel(updatedAt: number) {
  if (!updatedAt) return ""
  const date = new Date(updatedAt)
  return Number.isNaN(date.getTime()) ? "" : date.toLocaleDateString()
}

export function SessionSearchBar() {
  const engine = useEngine()
  let input: HTMLInputElement | undefined
  let timer: ReturnType<typeof setTimeout> | undefined

  // Titles come from what the engine has already hydrated. The sidebar asks for every workspace on
  // startup, so title search reuses that instead of issuing a listing of its own.
  const searchable = () =>
    workspaces().flatMap((workspace) =>
      sessionsFor(engine.state, workspace.path).map((session) => ({
        id: session.id,
        title: session.title || t("drift.thread.untitled"),
        directory: session.directory,
        updatedAt: (session.time.updated ?? 0) * 1000,
      })),
    )

  const run = createSessionSearchRunner({
    store: driftStore,
    sessions: searchable,
    workspaces: () =>
      workspaces().map((workspace) => ({ id: workspace.id, name: workspace.name, path: workspace.path })),
    archived: () => archivedIds(),
  })

  createEffect(() => {
    const value = query()
    const current = mode()
    setResults({ query: value, mode: current })
    if (timer !== undefined) clearTimeout(timer)
    if (!sessionSearchReady(value)) {
      setResults({ hits: [], loading: false, error: "" })
      return
    }
    // Title search is local and instant, but both modes wait out the same pause so results do not
    // reorder under a fast typist on the way to a content query.
    timer = setTimeout(() => void run(value, current, (next) => setResults(next)), sessionSearchDebounceMs)
  })
  onCleanup(() => {
    if (timer !== undefined) clearTimeout(timer)
  })

  return (
    <div class="shrink-0 px-2 pb-1.5">
      <div class="flex items-center gap-1.5 rounded-md border border-edge bg-raised/45 px-2 transition-colors focus-within:border-accent">
        <IconSearch class="size-3.5 shrink-0 text-ink-faint" />
        <input
          ref={input}
          class="min-w-0 flex-1 bg-transparent py-1.5 text-sm text-ink outline-none placeholder:text-ink-faint"
          placeholder={t("drift.search.sessions.placeholder")}
          value={query()}
          onInput={(event) => setQuery(event.currentTarget.value)}
          onKeyDown={(event) => {
            if (event.key !== "Escape") return
            event.preventDefault()
            setQuery("")
            input?.blur()
          }}
        />
        <Show when={query()}>
          <button
            class="flex size-5 shrink-0 items-center justify-center rounded text-ink-faint transition-colors hover:text-ink"
            title={t("drift.search.clear")}
            onClick={() => {
              setQuery("")
              input?.focus()
            }}
          >
            <IconX class="size-3" />
          </button>
        </Show>
      </div>
      <Show when={sessionSearchActive()}>
        <div class="mt-1.5 flex items-center gap-1 rounded-md border border-edge bg-overlay/40 p-0.5 text-[0.7rem]">
          <For each={["name", "content"] as SessionSearchMode[]}>
            {(option) => (
              <button
                class="flex-1 rounded px-2 py-1 transition-colors"
                classList={{
                  "bg-raised text-ink shadow-sm shadow-black/10": mode() === option,
                  "text-ink-faint hover:text-ink-muted": mode() !== option,
                }}
                onClick={() => setMode(option)}
              >
                {t(option === "name" ? "drift.search.mode.name" : "drift.search.mode.content")}
              </button>
            )}
          </For>
        </div>
      </Show>
    </div>
  )
}

/** The result list, rendered in place of the workspace tree while a search is active. */
export function SessionSearchResults() {
  function open(hit: SessionSearchHit) {
    if (hit.messageId) setPendingReveal({ sessionId: hit.sessionId, messageId: hit.messageId })
    if (hit.workspaceId) selectWorkspace(hit.workspaceId)
    selectSession(hit.sessionId)
  }

  return (
    <div class="space-y-0.5 px-1">
      <Show when={results.error}>
        <div class="px-2 py-3 text-xs text-danger">{results.error}</div>
      </Show>
      <Show when={results.loading && !results.hits.length}>
        <div class="px-2 py-3 text-xs text-ink-faint">{t("common.loading")}</div>
      </Show>
      <Show when={!results.loading && !results.error && !results.hits.length}>
        <div class="px-2 py-3 text-xs text-ink-faint">{t("drift.search.empty")}</div>
      </Show>
      <For each={results.hits}>
        {(hit) => (
          <button
            data-sidebar-navigation
            class="w-full rounded-md px-2 py-1.5 text-left transition-colors hover:bg-raised/60"
            onClick={() => open(hit)}
          >
            <div class="flex items-center gap-1.5">
              <span class="min-w-0 flex-1 truncate text-sm text-ink">{hit.title}</span>
              <Show when={hit.archived}>
                <span class="shrink-0 rounded-full border border-edge px-1.5 text-[0.6rem] text-ink-faint">
                  {t("drift.search.archived")}
                </span>
              </Show>
            </div>
            <div class="flex items-center gap-1.5 text-[0.65rem] text-ink-faint">
              <span class="min-w-0 truncate">{hit.workspaceName}</span>
              <Show when={updatedLabel(hit.updatedAt)}>
                <span class="shrink-0">{updatedLabel(hit.updatedAt)}</span>
              </Show>
            </div>
            <Show when={hit.excerpt}>
              {(excerpt) => (
                <div class="mt-0.5 line-clamp-2 text-[0.68rem] leading-snug text-ink-muted">
                  <For each={highlightSegments(excerpt(), results.query)}>
                    {(segment) => (
                      <span classList={{ "rounded bg-accent/25 text-ink": segment.match }}>{segment.text}</span>
                    )}
                  </For>
                </div>
              )}
            </Show>
          </button>
        )}
      </For>
    </div>
  )
}
