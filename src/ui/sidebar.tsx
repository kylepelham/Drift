import { closeMobileDrawer, mobileDrawerOpen, openMobileDrawer } from "../state/navigation";
import { createEffect, createSignal, For, onCleanup, Show, untrack } from "solid-js";
import { listenOpencodeImport, opencodeImport } from "../state/opencode-import";
import { archiveFailed, forkThread, WorkspaceGroup } from "./workspaces";
import { IconArchive, IconGear, IconPlus, IconSearch } from "./icons";
import { createMediaQuery, smallScreenQuery } from "../state/screen";
import { selectedSession, selectSession } from "../state/selection";
import { pickBackup, saveBackup } from "./session-backup";
import { emitThreadArchived } from "../plugins";
import { normalizeDir } from "../engine/store";
import { drawerSwipe } from "./drawer-swipe";
import { pickFolder } from "../state/dialog";
import { persisted } from "../state/persist";
import { openSettings } from "./settings";
import { ArchiveModal } from "./archive";
import { nativeFrame } from "./titlebar";
import { useEngine } from "../engine";
import { DriftLogo } from "./logo";
import { t } from "../state/i18n";
import {
    addWorkspace,
    archiveSession,
    removedWorkspaces,
    selectWorkspace,
    updateWorkspace,
    workspaces,
} from "../state/workspaces";
import {
    SessionSearchBar,
    SessionSearchResults,
    sessionSearchActive,
    sessionSearchOpen,
    toggleSessionSearch,
} from "./session-search";
import {
    SessionMenu,
    WorkspaceEditModal,
    WorkspaceMenu,
    type SessionMenuState,
    type WorkspaceMenuState,
} from "./workspace-dialogs";

import type { Workspace } from "../state/store";

const minSidebarWidth = 192;
const maxSidebarWidth = 480;
const [storedSidebarWidth, storeSidebarWidth] = persisted("drift.sidebar.width", 256);
const clampSidebarWidth = (width: number) => Math.min(maxSidebarWidth, Math.max(minSidebarWidth, width));

export function sidebarWidthFromDrag(startWidth: number, deltaX: number, scale: number) {
    return clampSidebarWidth(startWidth + deltaX / Math.max(scale, 0.01));
}

export function Sidebar() {
    const engine = useEngine();
    const narrow = createMediaQuery(smallScreenQuery);
    createEffect(() => {
        if (!narrow() && mobileDrawerOpen()) closeMobileDrawer();
    });
    const [menu, setMenu] = createSignal<WorkspaceMenuState | null>(null);
    const [sessionMenu, setSessionMenu] = createSignal<SessionMenuState | null>(null);
    const [editing, setEditing] = createSignal<string | null>(null);
    const [archive, setArchive] = createSignal(false);
    const [moveStatus, setMoveStatus] = createSignal<{ error: boolean; text: string } | null>(null);
    const [width, setWidth] = createSignal(clampSidebarWidth(storedSidebarWidth()));
    let resizeStartX = 0;
    let resizeStartWidth = untrack(width);
    let resizeScale = 1;

    async function add() {
        const path = await pickFolder();
        if (path) await addWorkspace(path);
    }

    async function moveSessionTo(state: SessionMenuState, destination: Workspace) {
        const selected = selectedSession();
        setMoveStatus({ error: false, text: t("drift.sidebar.movingSession", { workspace: destination.name }) });

        const result = await engine.actions.moveSession(state.sessionId, destination.path);
        if (!result.ok)
            return setMoveStatus({ error: true, text: result.error ?? t("drift.sidebar.moveSessionFailed") });

        if (selected && result.moved.includes(selected)) {
            selectWorkspace(destination.id);
            selectSession(selected);
        }

        setMoveStatus(null);
    }

    async function backupThread(sessionId: string) {
        const backup = await engine.actions.exportSessions([sessionId]);
        if (backup) await saved(saveBackup(backup, engine.state.sessions[sessionId]?.title ?? sessionId));
    }

    async function backupWorkspace(workspace: Workspace) {
        const backup = await engine.actions.exportWorkspace(workspace.path);
        if (backup) await saved(saveBackup(backup, workspace.name));
    }

    /** Says where a backup went, or why it could not be written. */
    async function saved(saving: Promise<string | undefined>) {
        const path = await saving.catch((cause: unknown) => setMoveStatus({ error: true, text: String(cause) }));
        if (typeof path !== "string") return;

        setMoveStatus({ error: false, text: t("drift.backup.saved", { path }) });
        setTimeout(() => setMoveStatus(null), 6000);
    }

    async function importInto(workspace: Workspace) {
        const backup = await pickBackup().catch(() => null);
        if (backup === null) return setMoveStatus({ error: true, text: t("drift.backup.unreadable") });
        if (!backup) return;

        setMoveStatus({ error: false, text: t("drift.backup.importing", { workspace: workspace.name }) });
        const restored = await engine.actions.importSessions(workspace.path, backup);
        if (!restored) return setMoveStatus(null);

        const counts = { imported: restored.imported.length, skipped: restored.skipped.length };
        setMoveStatus({ error: false, text: t("drift.backup.imported", counts) });
        setTimeout(() => setMoveStatus(null), 5000);
    }

    async function retargetWorkspace(workspace: Workspace) {
        const path = await pickFolder();
        if (!path || normalizeDir(path) === normalizeDir(workspace.path)) return;

        const collision = [...workspaces(), ...removedWorkspaces()].find(
            (entry) => entry.id !== workspace.id && normalizeDir(entry.path) === normalizeDir(path),
        );
        if (collision) {
            setMoveStatus({
                error: true,
                text: t("drift.sidebar.pathAlreadySaved", { path, workspace: collision.name }),
            });
            return;
        }

        setMoveStatus({
            error: false,
            text: t("drift.sidebar.movingWorkspaceSessions", { workspace: workspace.name }),
        });
        const result = await engine.actions.moveWorkspaceSessions(workspace.path, path);
        if (!result.ok)
            return setMoveStatus({ error: true, text: result.error ?? t("drift.sidebar.moveWorkspaceFailed") });
        try {
            await updateWorkspace(workspace.id, { path });
            setMoveStatus(null);
        } catch (error) {
            const rollback = await engine.actions.moveWorkspaceSessions(path, workspace.path);
            const detail = error instanceof Error ? error.message : String(error);
            setMoveStatus({
                error: true,
                text: rollback.ok
                    ? t("drift.sidebar.saveWorkspacePathFailed", { error: detail })
                    : t("drift.sidebar.workspaceMovePartial", { error: detail }),
            });
        }
    }

    function moveResize(event: PointerEvent) {
        const handle = event.currentTarget as HTMLElement;
        if (!handle.hasPointerCapture(event.pointerId)) return;

        setWidth(sidebarWidthFromDrag(resizeStartWidth, event.clientX - resizeStartX, resizeScale));
    }

    function finishResize() {
        document.body.style.cursor = "";
        document.body.style.userSelect = "";
        storeSidebarWidth(width());
    }

    function resizeWithKeyboard(event: KeyboardEvent) {
        const direction = resizeDirection(event.key);
        if (!direction) return;

        event.preventDefault();
        const next = clampSidebarWidth(width() + direction * 16);
        setWidth(next);
        storeSidebarWidth(next);
    }

    onCleanup(finishResize);

    return (
        <aside
            ref={(drawer) =>
                onCleanup(
                    drawerSwipe(drawer, {
                        open: mobileDrawerOpen,
                        show: openMobileDrawer,
                        hide: () => closeMobileDrawer(),
                        narrow,
                    }),
                )
            }
            class="app-sidebar relative flex shrink-0 flex-col border-r border-edge bg-sidebar"
            classList={{ "mobile-sidebar-open": mobileDrawerOpen() }}
            style={{ width: `${width()}px` }}
            onClick={(event) => {
                if (narrow() && event.target instanceof Element && event.target.closest("[data-sidebar-navigation]"))
                    queueMicrotask(() => closeMobileDrawer());
            }}
        >
            <div
                class="sidebar-top flex shrink-0 items-center justify-between pr-3.5 pl-4"
                classList={{ "h-11": nativeFrame(), "pt-2.5 pb-1.5": !nativeFrame() }}
                data-tauri-drag-region={nativeFrame() ? "" : undefined}
            >
                <Show
                    when={nativeFrame()}
                    fallback={
                        <span class="sidebar-heading min-w-0 truncate text-xs font-medium text-ink-faint">
                            {t("drift.sidebar.workspaces")}
                        </span>
                    }
                >
                    <div class="pointer-events-none flex min-w-0 items-center gap-2 text-ink-faint">
                        <DriftLogo class="size-4 shrink-0" />
                        <span class="truncate text-xs font-medium">Drift</span>
                    </div>
                </Show>
                <div class="flex items-center gap-0.5">
                    <button
                        class="flex size-7 items-center justify-center rounded-md transition-colors hover:bg-raised hover:text-ink"
                        classList={{ "text-ink": sessionSearchOpen(), "text-ink-faint": !sessionSearchOpen() }}
                        title={t("drift.search.sessions.placeholder")}
                        onClick={toggleSessionSearch}
                    >
                        <IconSearch class="size-3.5" />
                    </button>
                    <button
                        class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
                        title={t("drift.sidebar.archived")}
                        onClick={() => setArchive(true)}
                    >
                        <IconArchive />
                    </button>
                    <button
                        class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
                        title={t("drift.sidebar.addWorkspace")}
                        onClick={() => void add()}
                    >
                        <IconPlus />
                    </button>
                </div>
            </div>
            <Show when={sessionSearchOpen()}>
                <SessionSearchBar />
            </Show>
            <nav class="app-sidebar-scroll min-h-0 flex-1 space-y-3 overflow-y-auto overscroll-contain px-2 pb-2">
                <Show when={!sessionSearchActive()} fallback={<SessionSearchResults />}>
                    <For each={workspaces()}>
                        {(workspace) => (
                            <WorkspaceGroup workspace={workspace} onMenu={setMenu} onSessionMenu={setSessionMenu} />
                        )}
                    </For>
                    <Show when={workspaces().length === 0}>
                        <div class="px-2 py-4 text-xs text-ink-faint">{t("drift.sidebar.empty")}</div>
                    </Show>
                </Show>
            </nav>
            <SidebarFooter onSettings={openSettings} />
            <Show when={menuWorkspace()}>
                {(entry) => (
                    <WorkspaceMenu
                        state={entry().state}
                        workspace={entry().workspace}
                        onEdit={() => setEditing(entry().workspace.id)}
                        onMove={() => void retargetWorkspace(entry().workspace)}
                        onBackup={() => void backupWorkspace(entry().workspace)}
                        onImport={() => void importInto(entry().workspace)}
                        onClose={() => setMenu(null)}
                    />
                )}
            </Show>
            <Show when={sessionMenu()}>
                {(state) => (
                    <SessionMenu
                        state={state()}
                        workspaces={workspaces()}
                        onMove={(workspace) => void moveSessionTo(state(), workspace)}
                        onBackup={() => void backupThread(state().sessionId)}
                        onFork={() => void forkThread(engine, state().sessionId, state().workspaceId)}
                        onArchive={() => {
                            const id = state().sessionId;
                            if (selectedSession() === id) selectSession(null);
                            void archiveSession(id, state().workspaceId, engine.actions.setArchived)
                                .then(() => emitThreadArchived(id))
                                .catch((cause: unknown) => archiveFailed(engine, cause));
                        }}
                        onClose={() => setSessionMenu(null)}
                    />
                )}
            </Show>
            <Show when={editingWorkspace()}>
                {(workspace) => <WorkspaceEditModal workspace={workspace()} onClose={() => setEditing(null)} />}
            </Show>
            <Show when={archive()}>
                <ArchiveModal onClose={() => setArchive(false)} />
            </Show>
            <Show when={moveStatus()}>
                {(status) => (
                    <button
                        class="fixed bottom-4 left-1/2 z-50 max-w-lg -translate-x-1/2 rounded-lg border border-edge bg-overlay px-3 py-2 text-xs shadow-xl shadow-black/40"
                        classList={{ "text-danger": status().error, "text-ink-muted": !status().error }}
                        onClick={() => status().error && setMoveStatus(null)}
                    >
                        {status().text}
                    </button>
                )}
            </Show>
            <div
                role="separator"
                aria-label={t("drift.sidebar.resize")}
                aria-orientation="vertical"
                aria-valuemin={minSidebarWidth}
                aria-valuemax={maxSidebarWidth}
                aria-valuenow={Math.round(width())}
                tabIndex={0}
                class="sidebar-resizer absolute inset-y-0 right-0 z-20 w-1 translate-x-1/2 cursor-col-resize transition-colors hover:bg-accent/50 focus:bg-accent/50 focus:outline-none"
                onPointerDown={(event) => {
                    event.preventDefault();
                    const aside = event.currentTarget.parentElement;
                    const renderedWidth = aside?.getBoundingClientRect().width ?? width();
                    resizeStartX = event.clientX;
                    resizeStartWidth = width();
                    resizeScale = renderedWidth / Math.max(resizeStartWidth, 1);
                    event.currentTarget.setPointerCapture(event.pointerId);
                    document.body.style.cursor = "col-resize";
                    document.body.style.userSelect = "none";
                }}
                onPointerMove={moveResize}
                onPointerUp={(event) => {
                    event.currentTarget.releasePointerCapture(event.pointerId);
                }}
                onLostPointerCapture={finishResize}
                onKeyDown={resizeWithKeyboard}
            />
        </aside>
    );

    function menuWorkspace() {
        const state = menu();
        if (!state) return null;

        const workspace = workspaces().find((workspace) => workspace.id === state.workspaceId);

        return workspace ? { state, workspace } : null;
    }

    function editingWorkspace() {
        return workspaces().find((workspace) => workspace.id === editing()) ?? null;
    }
}

function SidebarFooter(props: { onSettings: () => void }) {
    listenOpencodeImport();

    return (
        <div class="shrink-0 px-2 py-2">
            <Show when={opencodeImport()}>
                {(progress) => (
                    <div
                        class="mb-1 px-2 py-1.5 text-[length:calc(0.7rem*var(--sidebar-scale,1))] text-ink-faint"
                        title={t("drift.import.description")}
                    >
                        <div class="flex justify-between gap-2">
                            <span class="truncate">{t("drift.import.progress")}</span>
                            <span class="tabular-nums">
                                {progress().done.toLocaleString()} / {progress().total.toLocaleString()}
                            </span>
                        </div>
                        <div class="mt-1 h-1 overflow-hidden rounded-full bg-raised">
                            <div
                                class="h-full bg-accent transition-[width]"
                                style={{ width: `${(progress().done / progress().total) * 100}%` }}
                            />
                        </div>
                    </div>
                )}
            </Show>
            <button
                data-sidebar-navigation
                class="flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-sm text-ink-muted transition-colors hover:bg-raised hover:text-ink"
                onClick={() => props.onSettings()}
            >
                <IconGear />
                <span>{t("sidebar.settings")}</span>
            </button>
        </div>
    );
}

function resizeDirection(key: string) {
    if (key === "ArrowLeft") return -1;
    if (key === "ArrowRight") return 1;

    return 0;
}
