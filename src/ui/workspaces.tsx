import { normalizeDir, sessionBusy, sessionsFor, taskForWorker, workerQueued } from "../engine/store";
import { cachedSessions, rememberSessions, type CachedSession } from "../state/session-cache";
import { ago, dayDividers, startOfDay, workspaceInitials } from "./workspace-presentation";
import { IconArchive, IconBranch, IconDots, IconSquarePen } from "./icons";
import { selectedSession, selectSession } from "../state/selection";
import { sidebarWorkers } from "../state/permission-attention";
import { sidebarDayDividers } from "../state/prefs";
import { useEngine, type Engine } from "../engine";
import { emitThreadArchived } from "../plugins";
import { TextShimmer } from "./text-shimmer";
import { dragReorder } from "./drag-reorder";
import { BackgroundTag } from "./task-dock";
import { t } from "../state/i18n";
import {
    activeWorkspaceId,
    archivedIds,
    archiveSession,
    moveWorkspace,
    selectWorkspace,
    toggleWorkspaceCollapsed,
    workspaceCollapsed,
} from "../state/workspaces";
import {
    createEffect,
    createMemo,
    createSignal,
    Match,
    onCleanup,
    onMount,
    Show,
    Switch,
    For,
    type JSX,
} from "solid-js";

import type { WorkspaceMenuState, SessionMenuState } from "./workspace-dialogs";
import type { Workspace } from "../state/store";

const sessionPageSize = 5;

const threadRow = (session: { id: string; title: string; updatedAt: number }): CachedSession => ({
    id: session.id,
    title: session.title,
    updated: session.updatedAt,
});

export function WorkspaceGroup(props: {
    workspace: Workspace;
    onMenu: (state: WorkspaceMenuState) => void;
    onSessionMenu: (state: SessionMenuState) => void;
}) {
    const engine = useEngine();
    let root!: HTMLDivElement;
    let cancelDrag = () => {};
    onCleanup(() => cancelDrag());

    const collapsed = () => workspaceCollapsed(props.workspace.id);
    const [visibleCount, setVisibleCount] = createSignal(sessionPageSize);
    const active = () => activeWorkspaceId() === props.workspace.id;
    const online = () => engine.state.connection === "online";
    const live = createMemo(() => sessionsFor(engine.state, props.workspace.path).map(threadRow));
    const authoritative = () =>
        online() &&
        (engine.state.sessionSnapshotAll ||
            normalizeDir(engine.state.sessionSnapshotDirectory) === normalizeDir(props.workspace.path));
    // Only a complete scoped snapshot may clear the cache; online is reported before hydration ends.
    createEffect(() => {
        const current = live();
        if (current.length || authoritative()) rememberSessions(props.workspace.path, current);
    });

    // Cold engine startup takes seconds; the last known threads stand in until it answers.
    const all = createMemo(() => {
        const current = live();
        if (current.length || authoritative()) return current;
        return cachedSessions(props.workspace.path);
    });
    const children = (parentId: string) => sidebarWorkers(engine.state, parentId, selectedSession());
    const sessions = createMemo(() => all().filter((session) => !archivedIds().has(session.id)));
    const visibleSessions = createMemo(() => sessions().slice(0, visibleCount()));
    // Rows are keyed by id; row objects are rebuilt on every session update and would remount the DOM.
    const visibleIds = createMemo(() => visibleSessions().map((session) => session.id), [], {
        equals: (a, b) => a.length === b.length && a.every((id, i) => id === b[i]),
    });
    const rowFor = (id: string) => visibleSessions().find((session) => session.id === id);
    const remaining = createMemo(() => Math.max(0, sessions().length - visibleSessions().length));
    // Bumped at each local midnight, so yesterday's "Today" heading moves on without a restart.
    const [day, setDay] = createSignal(Date.now());
    let midnight: ReturnType<typeof setTimeout> | undefined;

    const nextMidnight = () => {
        midnight = setTimeout(
            () => {
                setDay(Date.now());
                nextMidnight();
            },
            startOfDay(Date.now()) + 86_400_000 + 1_000 - Date.now(),
        );
    };

    onMount(nextMidnight);
    onCleanup(() => clearTimeout(midnight));

    const dividers = createMemo(() =>
        sidebarDayDividers() ? dayDividers(visibleSessions(), day()) : new Map<string, string>(),
    );
    const openMenu = (x: number, y: number) => props.onMenu({ x, y, workspaceId: props.workspace.id });

    return (
        <div ref={root} data-workspace={props.workspace.id}>
            <div
                class="group sticky top-0 z-[1] flex w-full cursor-pointer items-center gap-2.5 rounded-md py-1.5 pr-1.5 pl-2 transition-colors"
                classList={{ "bg-raised/55": active(), "bg-surface hover:bg-raised/45": !active() }}
                onPointerDown={(event) => {
                    cancelDrag();
                    cancelDrag = dragReorder(event, root, {
                        selector: ":scope > [data-workspace]",
                        id: props.workspace.id,
                        itemID: (element) => element.dataset.workspace ?? "",
                        move: moveWorkspace,
                        dragged: markWorkspaceDragged,
                    });
                }}
                onClick={() => {
                    if (dragged) return;
                    toggleWorkspaceCollapsed(props.workspace.id);
                }}
                onContextMenu={(event) => {
                    event.preventDefault();
                    openMenu(event.clientX, event.clientY);
                }}
            >
                <button
                    title={collapsed() ? t("drift.workspace.showThreads") : t("drift.workspace.hideThreads")}
                    aria-expanded={!collapsed()}
                    class="flex shrink-0 rounded-[5px]"
                    onPointerDown={(event) => event.stopPropagation()}
                    onClick={(event) => {
                        event.stopPropagation();
                        toggleWorkspaceCollapsed(props.workspace.id);
                    }}
                >
                    <WorkspaceIcon workspace={props.workspace} />
                </button>
                <span
                    class="min-w-0 flex-1 truncate text-sm"
                    classList={{ "text-ink": active(), "text-ink-muted": !active() }}
                >
                    {props.workspace.name}
                </span>
                <div class="flex items-center" classList={{ "invisible group-hover:visible": !active() }}>
                    <RowButton
                        title={t("common.moreOptions")}
                        onClick={(event) => {
                            const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
                            openMenu(rect.left, rect.bottom + 4);
                        }}
                    >
                        <IconDots />
                    </RowButton>
                    <RowButton
                        title={t("drift.thread.new")}
                        navigation
                        onClick={() => {
                            selectWorkspace(props.workspace.id);
                            selectSession(null);
                        }}
                    >
                        <IconSquarePen />
                    </RowButton>
                </div>
            </div>
            <Show when={!collapsed()}>
                <div class="mt-0.5 space-y-0.5">
                    <For each={visibleIds()}>
                        {(id) => (
                            <>
                                <Show when={dividers().get(id)}>
                                    {(label) => (
                                        <div class="flex items-center gap-2 px-2 pt-2 pb-0.5 text-[0.65rem] font-medium text-ink-faint select-none first:pt-0.5">
                                            <span class="shrink-0">{label()}</span>
                                            <span class="h-px flex-1 bg-edge" />
                                        </div>
                                    )}
                                </Show>
                                <ThreadItem
                                    sessionId={id}
                                    title={rowFor(id)?.title ?? ""}
                                    updated={rowFor(id)?.updated ?? 0}
                                    workspace={props.workspace}
                                    onMenu={props.onSessionMenu}
                                />
                                <For each={children(id)}>
                                    {(child) => (
                                        <ChildThreadItem
                                            sessionId={child.id}
                                            title={child.title}
                                            workspace={props.workspace}
                                            onMenu={props.onSessionMenu}
                                        />
                                    )}
                                </For>
                            </>
                        )}
                    </For>
                    <Show when={remaining() > 0 || visibleCount() > sessionPageSize}>
                        <div class="flex items-center">
                            <Show when={remaining() > 0}>
                                <button
                                    class="flex h-7 min-w-0 flex-1 items-center rounded-md px-2 text-left text-[0.72rem] text-ink-faint transition-colors hover:bg-raised/60 hover:text-ink-muted"
                                    onClick={() =>
                                        setVisibleCount((count) => Math.min(count + sessionPageSize, sessions().length))
                                    }
                                >
                                    {t("drift.thread.loadMore", { count: Math.min(sessionPageSize, remaining()) })}
                                </button>
                            </Show>
                            <Show when={visibleCount() > sessionPageSize}>
                                <button
                                    class="flex h-7 shrink-0 items-center rounded-md px-2 text-[0.72rem] text-ink-faint transition-colors hover:bg-raised/60 hover:text-ink-muted"
                                    classList={{ "flex-1 text-left": remaining() === 0 }}
                                    onClick={() => setVisibleCount(sessionPageSize)}
                                >
                                    {t("drift.thread.showLess")}
                                </button>
                            </Show>
                        </div>
                    </Show>
                    <Show when={sessions().length === 0 && active() && !authoritative()}>
                        <div class="px-2 py-1.5 text-xs text-ink-faint" role="status" aria-live="polite">
                            <TextShimmer text={t("common.loading")} />
                        </div>
                    </Show>
                    <Show when={sessions().length === 0 && active() && authoritative()}>
                        <div class="px-2 py-1.5 text-xs text-ink-faint">{t("drift.thread.empty")}</div>
                    </Show>
                </div>
            </Show>
        </div>
    );
}

let dragged = false;

function markWorkspaceDragged() {
    dragged = true;
    setTimeout(() => (dragged = false), 0);
}

function RowButton(props: {
    title: string;
    navigation?: boolean;
    disabled?: boolean;
    onClick: (event: MouseEvent) => void;
    children: JSX.Element;
}) {
    return (
        <button
            title={props.title}
            disabled={props.disabled}
            data-sidebar-navigation={props.navigation ? "" : undefined}
            class="flex size-7 shrink-0 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-overlay hover:text-ink disabled:cursor-wait disabled:opacity-40"
            onPointerDown={(event) => event.stopPropagation()}
            onClick={(event) => {
                event.stopPropagation();
                props.onClick(event);
            }}
        >
            {props.children}
        </button>
    );
}

function ThreadItem(props: {
    sessionId: string;
    title: string;
    updated: number;
    workspace: Workspace;
    onMenu: (state: SessionMenuState) => void;
}) {
    const engine = useEngine();
    const active = () => selectedSession() === props.sessionId;
    const [forking, setForking] = createSignal(false);

    return (
        <div
            data-sidebar-navigation
            class="group flex h-8 cursor-pointer items-center gap-2 rounded-md py-1 pr-1 pl-2 transition-colors"
            classList={{ "bg-raised/80": active(), "hover:bg-raised/45": !active() }}
            onClick={() => {
                selectWorkspace(props.workspace.id);
                selectSession(props.sessionId);
            }}
            onContextMenu={(event) => {
                event.preventDefault();
                props.onMenu({
                    x: event.clientX,
                    y: event.clientY,
                    sessionId: props.sessionId,
                    workspaceId: props.workspace.id,
                });
            }}
        >
            <StatusDot sessionId={props.sessionId} />
            <span
                class="min-w-0 flex-1 truncate text-[0.84rem]"
                classList={{ "text-ink": active(), "text-ink-muted": !active() }}
            >
                {props.title || t("drift.thread.untitled")}
            </span>
            <span class="thread-age shrink-0 text-[0.65rem] text-ink-faint/75 group-hover:hidden">
                {ago(props.updated)}
            </span>
            <span class="thread-hover-actions hidden shrink-0 items-center group-hover:flex">
                <RowButton
                    title={forking() ? t("common.loading") : t("drift.slash.fork.active.description")}
                    disabled={forking()}
                    onClick={() => {
                        if (forking()) return;

                        setForking(true);
                        void forkThread(engine, props.sessionId, props.workspace.id).finally(() => setForking(false));
                    }}
                >
                    <IconBranch />
                </RowButton>
                <RowButton
                    title={t("command.session.archive")}
                    onClick={() => {
                        if (selectedSession() === props.sessionId) selectSession(null);
                        void archiveSession(props.sessionId, props.workspace.id, engine.actions.setArchived)
                            .then(() => emitThreadArchived(props.sessionId))
                            .catch((cause: unknown) => archiveFailed(engine, cause));
                    }}
                >
                    <IconArchive />
                </RowButton>
            </span>
            <span class="touch-thread-menu hidden shrink-0">
                <RowButton
                    title={t("common.moreOptions")}
                    onClick={(event) => {
                        const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
                        props.onMenu({
                            x: rect.left,
                            y: rect.bottom,
                            sessionId: props.sessionId,
                            workspaceId: props.workspace.id,
                        });
                    }}
                >
                    <IconDots />
                </RowButton>
            </span>
        </div>
    );
}

/** The engine refused to archive or restore; the thread stays where it was. */
export function archiveFailed(engine: Engine, cause: unknown) {
    engine.actions.notice({
        title: t("command.session.archive"),
        message: cause instanceof Error ? cause.message : String(cause),
        variant: "error",
    });
}

function StatusDot(props: { sessionId: string }) {
    const engine = useEngine();
    const permissions = () => engine.state.permissions[props.sessionId] ?? [];
    const attention = () => permissions().length > 0 || (engine.state.questions[props.sessionId]?.length ?? 0) > 0;
    const attentionTitle = () =>
        permissions().length > 0 ? t("drift.thread.waitingForPermission") : t("drift.thread.waitingForAnswer");

    return (
        <Switch>
            <Match when={attention()}>
                <span class="size-1.5 shrink-0 rounded-full bg-warn" title={attentionTitle()} />
            </Match>
            <Match when={sessionBusy(engine.state, props.sessionId)}>
                <span class="pulse-soft size-1.5 shrink-0 rounded-full bg-accent" title={t("drift.thread.working")} />
            </Match>
            <Match when={workerQueued(engine.state, props.sessionId)}>
                <span
                    class="size-1.5 shrink-0 rounded-full border border-accent/70"
                    title={t("drift.task.queued.description")}
                />
            </Match>
            <Match when={engine.state.errors[props.sessionId]}>
                <span class="size-1.5 shrink-0 rounded-full bg-danger" title={t("notification.session.error.title")} />
            </Match>
        </Switch>
    );
}

function ChildThreadItem(props: {
    sessionId: string;
    title: string;
    workspace: Workspace;
    onMenu: (state: SessionMenuState) => void;
}) {
    const engine = useEngine();
    const active = () => selectedSession() === props.sessionId;
    const background = () => taskForWorker(engine.state, props.sessionId)?.mode === "background";

    return (
        <div
            data-sidebar-navigation
            class="flex h-7 cursor-pointer items-center gap-1.5 rounded-md py-0.5 pr-2 pl-5 transition-colors"
            classList={{ "bg-raised": active(), "hover:bg-raised/60": !active() }}
            onClick={() => {
                selectWorkspace(props.workspace.id);
                selectSession(props.sessionId);
            }}
            onContextMenu={(event) => {
                event.preventDefault();
                props.onMenu({
                    x: event.clientX,
                    y: event.clientY,
                    sessionId: props.sessionId,
                    workspaceId: props.workspace.id,
                });
            }}
        >
            <span class="text-[0.7rem]" classList={{ "text-accent/70": background(), "text-ink-faint": !background() }}>
                &#8627;
            </span>
            <StatusDot sessionId={props.sessionId} />
            <span
                class="min-w-0 flex-1 truncate text-[0.75rem]"
                classList={{ "text-ink": active(), "text-ink-faint": !active() }}
            >
                {props.title || t("drift.thread.untitled")}
            </span>
            <Show when={background()}>
                <BackgroundTag />
            </Show>
            <span class="touch-thread-menu hidden shrink-0">
                <RowButton
                    title={t("common.moreOptions")}
                    onClick={(event) => {
                        const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
                        props.onMenu({
                            x: rect.left,
                            y: rect.bottom,
                            sessionId: props.sessionId,
                            workspaceId: props.workspace.id,
                        });
                    }}
                >
                    <IconDots />
                </RowButton>
            </span>
        </div>
    );
}

const hues = [212, 262, 330, 24, 96, 168];

/** Forks a thread already being forked, once: a repeated tap waits for the first. */
const forksUnderWay = new Map<string, Promise<void>>();

/**
 * Forks a thread and opens the copy in the row's workspace, but only if the user is still where they were when they
 * asked; until then nothing moves, so a failed fork leaves them on the thread they had open. Both the row's button
 * and its menu fork through here.
 */
export function forkThread(engine: Engine, sessionId: string, workspaceId: string) {
    const underWay = forksUnderWay.get(sessionId);
    if (underWay) return underWay;

    const from = activeWorkspaceId();
    const selection = selectedSession();
    const forking = engine.actions
        .fork(sessionId)
        .then((session) => {
            if (!session || activeWorkspaceId() !== from || selectedSession() !== selection) return;
            selectWorkspace(workspaceId);
            selectSession(session.id);
        })
        .finally(() => forksUnderWay.delete(sessionId));
    forksUnderWay.set(sessionId, forking);

    return forking;
}

export function WorkspaceIcon(props: { workspace: Workspace }) {
    const hue = () => {
        let hash = 0;
        for (const char of props.workspace.path) hash = (hash * 31 + char.charCodeAt(0)) | 0;
        return hues[Math.abs(hash) % hues.length];
    };
    return (
        <Show
            when={props.workspace.icon.startsWith("data:")}
            fallback={
                <span
                    class="workspace-icon flex size-5 shrink-0 items-center justify-center rounded-[5px] text-[0.58rem] font-semibold text-white/85"
                    style={{ background: `hsl(${hue()} 24% 31%)` }}
                >
                    {workspaceInitials(props.workspace.name)}
                </span>
            }
        >
            <img src={props.workspace.icon} alt="" class="workspace-icon size-5 shrink-0 rounded-[5px] object-cover" />
        </Show>
    );
}
