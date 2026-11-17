import { createEffect, createSignal, For, on, onCleanup, onMount, Show } from "solid-js";
import { archivedIds, selectWorkspace, workspaces } from "../state/workspaces";
import { comboFor, formatCombo, runKeybind } from "../state/keybinds";
import { findInConversation } from "./transcript-find";
import { selectSession } from "../state/selection";
import { showSidebar } from "../state/navigation";
import { sessionsFor } from "../engine/store";
import { createStore } from "solid-js/store";
import { driftStore } from "../state/store";
import { IconSearch, IconX } from "./icons";
import { useEngine } from "../engine";
import { t } from "../state/i18n";
import {
    createSessionSearchRunner,
    highlightSegments,
    sessionSearchDebounceMs,
    sessionSearchReady,
    type SessionSearchHit,
    type SessionSearchMode,
    type SessionSearchState,
} from "../state/session-search";

// Shared by the search input and the result list that replaces the workspace tree.
const [expanded, setExpanded] = createSignal(false);
const [query, setQuery] = createSignal("");
const [mode, setMode] = createSignal<SessionSearchMode>("name");
const [results, setResults] = createStore<SessionSearchState>({
    query: "",
    mode: "name",
    hits: [],
    loading: false,
    error: "",
});

// The result Enter opens; arrow keys in the search field move it.
const [active, setActive] = createSignal(0);
let searchInput: HTMLInputElement | undefined;

/** Set when a content result is opened, so the transcript can scroll to the message that matched. */
const [pendingReveal, setPendingReveal] = createSignal<{ sessionId: string; messageId: string } | null>(null);

export function sessionSearchActive() {
    return sessionSearchReady(query());
}

export function sessionSearchOpen() {
    return expanded();
}

function closeSessionSearch() {
    setExpanded(false);
    setQuery("");
}

/** The sidebar shows only a magnifier until this expands it into the input. */
export function toggleSessionSearch() {
    if (expanded()) {
        closeSessionSearch();
        return;
    }
    setExpanded(true);
}

/** The search shortcut opens the search, and pressed again in it switches between names and content. */
export function sessionSearchShortcut() {
    showSidebar();
    if (!expanded()) {
        setExpanded(true);
        return;
    }
    if (searchInput && document.activeElement === searchInput) {
        setMode(mode() === "name" ? "content" : "name");
        return;
    }
    searchInput?.focus();
    searchInput?.select();
}

function openHit(hit: SessionSearchHit) {
    if (hit.messageId) {
        setPendingReveal({ sessionId: hit.sessionId, messageId: hit.messageId });
        findInConversation(hit.sessionId, results.query.trim(), hit.messageId);
    }
    if (hit.workspaceId) selectWorkspace(hit.workspaceId);
    selectSession(hit.sessionId);
}

function searchKey(event: KeyboardEvent) {
    const count = results.hits.length;
    if (event.key === "Escape") {
        closeSessionSearch();
        runKeybind("focusComposer");
    } else if (event.key === "ArrowDown" && count) setActive((active() + 1) % count);
    else if (event.key === "ArrowUp" && count) setActive((active() - 1 + count) % count);
    else if (event.key === "Enter" && results.hits[active()]) openHit(results.hits[active()]);
    else return;
    event.preventDefault();
}

export function revealTarget(sessionId: string) {
    const pending = pendingReveal();
    return pending?.sessionId === sessionId ? pending.messageId : undefined;
}

export function clearReveal() {
    setPendingReveal(null);
}

function updatedLabel(updatedAt: number) {
    if (!updatedAt) return "";
    const date = new Date(updatedAt);
    return Number.isNaN(date.getTime()) ? "" : date.toLocaleDateString();
}

export function SessionSearchBar() {
    const engine = useEngine();
    let timer: ReturnType<typeof setTimeout> | undefined;

    onMount(() => searchInput?.focus());
    onCleanup(() => (searchInput = undefined));

    // Titles come from already hydrated sessions; the sidebar lists every workspace at startup.
    const searchable = () =>
        workspaces().flatMap((workspace) =>
            sessionsFor(engine.state, workspace.path).map((session) => ({
                id: session.id,
                title: session.title || t("drift.thread.untitled"),
                directory: session.directory,
                updatedAt: (session.updatedAt ?? 0) * 1000,
            })),
        );

    const run = createSessionSearchRunner({
        store: driftStore,
        sessions: searchable,
        workspaces: () =>
            workspaces().map((workspace) => ({ id: workspace.id, name: workspace.name, path: workspace.path })),
        archived: () => archivedIds(),
    });

    createEffect(() => {
        const value = query();
        const current = mode();
        setResults({ query: value, mode: current });
        if (timer !== undefined) clearTimeout(timer);
        if (!sessionSearchReady(value)) {
            setResults({ hits: [], loading: false, error: "" });
            return;
        }
        // Both modes share one pause so results do not reorder under a fast typist.
        timer = setTimeout(() => void run(value, current, (next) => setResults(next)), sessionSearchDebounceMs);
    });
    onCleanup(() => {
        if (timer !== undefined) clearTimeout(timer);
    });

    const switchHint = () => {
        const combo = comboFor("searchSessions");
        return combo ? `${t("drift.search.mode.switch")} (${formatCombo(combo)})` : t("drift.search.mode.switch");
    };

    return (
        <div class="shrink-0 px-2 pb-1.5">
            <div class="flex items-center gap-1.5 rounded-md border border-edge bg-raised/45 pr-1 pl-2 transition-colors focus-within:border-accent">
                <IconSearch class="size-3.5 shrink-0 text-ink-faint" />
                <input
                    ref={(element) => (searchInput = element)}
                    class="min-w-0 flex-1 bg-transparent py-1.5 text-sm text-ink outline-none placeholder:text-ink-faint"
                    placeholder={t(
                        mode() === "name" ? "drift.search.name.placeholder" : "drift.search.content.placeholder",
                    )}
                    aria-label={t("drift.search.sessions.placeholder")}
                    role="combobox"
                    aria-expanded={sessionSearchActive()}
                    aria-controls="session-search-results"
                    aria-activedescendant={results.hits.length ? `session-search-${active()}` : undefined}
                    value={query()}
                    onInput={(event) => setQuery(event.currentTarget.value)}
                    onKeyDown={searchKey}
                />
                <Show when={query()}>
                    <button
                        class="flex size-5 shrink-0 items-center justify-center rounded text-ink-faint transition-colors hover:text-ink"
                        title={t("drift.search.clear")}
                        onClick={() => {
                            setQuery("");
                            searchInput?.focus();
                        }}
                    >
                        <IconX class="size-3" />
                    </button>
                </Show>
                <div
                    class="flex shrink-0 rounded bg-overlay/60 p-px text-[length:calc(0.62rem*var(--ui-text-scale,1))]"
                    role="radiogroup"
                    aria-label={switchHint()}
                    title={switchHint()}
                >
                    <For each={["name", "content"] as SessionSearchMode[]}>
                        {(option) => (
                            <button
                                role="radio"
                                aria-checked={mode() === option}
                                tabIndex={-1}
                                class="rounded-[3px] px-1.5 py-0.5 transition-colors"
                                classList={{
                                    "bg-raised text-ink shadow-sm shadow-black/10": mode() === option,
                                    "text-ink-faint hover:text-ink-muted": mode() !== option,
                                }}
                                onPointerDown={(event) => event.preventDefault()}
                                onClick={() => {
                                    setMode(option);
                                    searchInput?.focus();
                                }}
                            >
                                {t(option === "name" ? "drift.search.mode.name" : "drift.search.mode.content")}
                            </button>
                        )}
                    </For>
                </div>
            </div>
        </div>
    );
}

/** The result list, rendered in place of the workspace tree while a search is active. */
export function SessionSearchResults() {
    let list!: HTMLDivElement;
    createEffect(
        on(
            () => results.hits,
            () => setActive(0),
        ),
    );
    createEffect(() => list.querySelector(`#session-search-${active()}`)?.scrollIntoView({ block: "nearest" }));

    return (
        <div ref={list} id="session-search-results" role="listbox" class="space-y-0.5 px-1">
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
                {(hit, index) => (
                    <button
                        id={`session-search-${index()}`}
                        role="option"
                        aria-selected={active() === index()}
                        tabIndex={-1}
                        data-sidebar-navigation
                        data-sidebar-row="thread"
                        data-thread-row={hit.sessionId}
                        data-workspace-id={hit.workspaceId}
                        class="w-full rounded-md px-2 py-1.5 text-left transition-colors"
                        classList={{
                            "bg-raised/70": active() === index(),
                            "hover:bg-raised/45": active() !== index(),
                        }}
                        onMouseEnter={() => setActive(index())}
                        onClick={() => openHit(hit)}
                    >
                        <div class="flex items-center gap-1.5">
                            <span class="min-w-0 flex-1 truncate text-sm text-ink">{hit.title}</span>
                            <Show when={hit.archived}>
                                <span class="shrink-0 rounded-full border border-edge px-1.5 text-[length:calc(0.6rem*var(--ui-text-scale,1))] text-ink-faint">
                                    {t("drift.search.archived")}
                                </span>
                            </Show>
                        </div>
                        <div class="flex items-center gap-1.5 text-[length:calc(0.65rem*var(--ui-text-scale,1))] text-ink-faint">
                            <span class="min-w-0 truncate">{hit.workspaceName}</span>
                            <Show when={updatedLabel(hit.updatedAt)}>
                                <span class="shrink-0">{updatedLabel(hit.updatedAt)}</span>
                            </Show>
                        </div>
                        <Show when={hit.excerpt}>
                            {(excerpt) => (
                                <div class="mt-0.5 line-clamp-2 text-[length:calc(0.68rem*var(--ui-text-scale,1))] leading-snug text-ink-muted">
                                    <For each={highlightSegments(excerpt(), results.query)}>
                                        {(segment) => (
                                            <span classList={{ "rounded bg-accent/25 text-ink": segment.match }}>
                                                {segment.text}
                                            </span>
                                        )}
                                    </For>
                                </div>
                            )}
                        </Show>
                    </button>
                )}
            </For>
        </div>
    );
}
