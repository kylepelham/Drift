import { openTranscriptFind, TranscriptFindBar, transcriptFindOpen } from "./transcript-find";
import { IconArrowUp, IconBranch, IconDiff, IconGlobe, IconMenu, IconSearch } from "./icons";
import { closeReview, openReview, reviewPanel, rightPanelOpen } from "../state/panels";
import { createMediaQuery, smallScreenQuery, touchInput } from "../state/screen";
import { selectedSession, selectSession } from "../state/selection";
import { createSignal, onCleanup, Show } from "solid-js";
import { nativeFrame, WindowControls } from "./titlebar";
import { openMobileDrawer } from "../state/navigation";
import { hiddenParent } from "../engine/sessions";
import { ContextMeter } from "./context-meter";
import { useEngine } from "../engine";
import { t } from "../state/i18n";
import {
    browserPaneCollapsed,
    browserPaneShowing,
    collapseBrowserPane,
    expandBrowserPane,
} from "../state/browser-pane";

const chatColumnWidth = 768;

function toggleReview() {
    if (reviewPanel()) closeReview();
    else openReview({ source: { kind: "uncommitted" } });
}

export function ChatHeader() {
    const engine = useEngine();
    const session = () => engine.state.sessions[selectedSession() ?? ""];
    const [transparent, setTransparent] = createSignal(false);
    const narrow = createMediaQuery(smallScreenQuery);
    let row!: HTMLDivElement;

    function remeasure() {
        const left = row.firstElementChild?.getBoundingClientRect().width ?? 0;
        const right = row.lastElementChild?.getBoundingClientRect().width ?? 0;
        const gap = Math.max(0, (row.clientWidth - chatColumnWidth) / 2);
        setTransparent(gap > left + 16 && gap > right + 16);
    }

    function observe(element: HTMLDivElement) {
        row = element;

        const observer = new ResizeObserver(remeasure);
        observer.observe(element);
        for (const child of element.children) observer.observe(child);
        onCleanup(() => observer.disconnect());
    }

    const backTarget = () => {
        const current = session();
        if (!current) return undefined;
        return hiddenParent(current) ?? engine.state.links[current.id];
    };
    return (
        <Show
            when={session() || nativeFrame()}
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
                data-tauri-drag-region={nativeFrame() ? "" : undefined}
                class="chat-header absolute inset-x-0 top-0 z-10 flex h-11 items-center gap-2 border-b px-4 transition-colors"
                classList={{
                    "chat-header-searching": transcriptFindOpen(),
                    "pointer-events-none": !nativeFrame(),
                    "border-edge/60 bg-bg": !transparent(),
                    "border-transparent bg-transparent": transparent(),
                }}
            >
                <div
                    class="chat-header-title pointer-events-auto flex min-w-0 max-w-[60%] items-center gap-2"
                    data-tauri-drag-region={nativeFrame() ? "" : undefined}
                >
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
                                <Show when={current().worktree}>
                                    {(name) => (
                                        <span
                                            class="flex min-w-0 shrink items-center gap-1 rounded-md border border-edge px-1.5 py-0.5 text-xs text-ink-muted"
                                            title={t("drift.worktree.branch")}
                                        >
                                            <IconBranch class="size-3 shrink-0" />
                                            <span class="truncate">drift/{name()}</span>
                                        </span>
                                    )}
                                </Show>
                            </>
                        )}
                    </Show>
                </div>
                <div
                    class="min-w-4 flex-1 self-stretch"
                    classList={{ "pointer-events-none": !nativeFrame() }}
                    data-tauri-drag-region={nativeFrame() ? "" : undefined}
                />
                <div
                    class="chat-header-tools pointer-events-auto flex shrink-0 items-center gap-2 self-stretch"
                    data-tauri-drag-region={nativeFrame() ? "" : undefined}
                >
                    <Show when={session()}>
                        {(current) => (
                            <>
                                <TranscriptFindBar />
                                <Show when={!transcriptFindOpen()}>
                                    <button
                                        class="flex size-7 shrink-0 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
                                        title={t("drift.search.transcript")}
                                        onClick={openTranscriptFind}
                                    >
                                        <IconSearch class="size-3.5" />
                                    </button>
                                    <button
                                        class="flex size-7 shrink-0 items-center justify-center rounded-md transition-colors hover:bg-raised hover:text-ink"
                                        classList={{ "text-accent": !!reviewPanel(), "text-ink-faint": !reviewPanel() }}
                                        title={t("drift.review.open")}
                                        aria-pressed={!!reviewPanel()}
                                        onClick={toggleReview}
                                    >
                                        <IconDiff class="size-3.5" />
                                    </button>
                                    <Show when={engine.state.browsers[current().id]}>
                                        <BrowserToggle sessionId={current().id} />
                                    </Show>
                                </Show>
                                <ContextMeter sessionId={current().id} />
                            </>
                        )}
                    </Show>
                    <Show when={(!rightPanelOpen() && !browserPaneShowing()) || narrow()}>
                        <WindowControls />
                    </Show>
                </div>
            </div>
        </Show>
    );
}

/** Shows or hides the conversation's browser pane; the browser keeps running either way. */
function BrowserToggle(props: { sessionId: string }) {
    const open = () => !browserPaneCollapsed(props.sessionId);

    return (
        <button
            class="flex size-7 shrink-0 items-center justify-center rounded-md transition-colors hover:bg-raised hover:text-ink"
            classList={{ "text-accent": open(), "text-ink-faint": !open() }}
            title={t(open() ? "drift.browser.hide" : "drift.browser.show")}
            aria-pressed={open()}
            onClick={() => (open() ? collapseBrowserPane(props.sessionId) : expandBrowserPane(props.sessionId))}
        >
            <IconGlobe class="size-3.5" />
        </button>
    );
}

function Title(props: { id: string; title: string }) {
    const engine = useEngine();
    const [editing, setEditing] = createSignal(false);

    const commit = (value: string) => {
        const next = value.trim();
        if (next && next !== props.title) void engine.actions.rename(props.id, next);
        setEditing(false);
    };

    return (
        <Show
            when={editing()}
            fallback={
                <span
                    class="min-w-0 cursor-text truncate text-sm text-ink"
                    title={t(touchInput() ? "common.edit" : "drift.thread.renameHint")}
                    role={touchInput() ? "button" : undefined}
                    tabIndex={touchInput() ? 0 : undefined}
                    onClick={() => touchInput() && setEditing(true)}
                    onKeyDown={(event) => {
                        if (touchInput() && (event.key === "Enter" || event.key === " ")) {
                            event.preventDefault();
                            setEditing(true);
                        }
                    }}
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
                    if (event.key === "Enter") commit(event.currentTarget.value);
                    if (event.key === "Escape") setEditing(false);
                }}
                onBlur={(event) => commit(event.currentTarget.value)}
            />
        </Show>
    );
}
