import { appKeyFree, onKeybind, sessionDigit, type KeybindAction } from "../state/keybinds";
import { nthSession, stepAround, VisitHistory } from "../state/session-nav";
import { actionFailed, archiveThread, forkThread } from "./workspaces";
import { selectedSession, selectSession } from "../state/selection";
import { normalizeDir, sessionsFor } from "../engine/store";
import { createEffect, onCleanup, untrack } from "solid-js";
import { showSidebar } from "../state/navigation";
import { requestRename } from "./header";
import { t } from "../state/i18n";
import {
    activeWorkspaceId,
    archivedIds,
    pickWorkspace,
    selectWorkspace,
    toggleWorkspaceCollapsed,
    workspaceCollapsed,
    workspaces,
} from "../state/workspaces";

import type { Workspace } from "../state/store";
import type { Engine } from "../engine";

/** The shortcuts that move between chats and workspaces, and act on the open chat. */
export function bindSessionKeys(engine: Engine, options: { openArchive: () => void }) {
    const history = new VisitHistory();
    const lastVisited = new Map<string, string>();
    createEffect(() => {
        const id = selectedSession();
        if (!id) return;
        history.visit(id);
        const workspace = untrack(activeWorkspaceId);
        if (workspace) lastVisited.set(workspace, id);
    });

    const live = (id: string) => !!engine.state.sessions[id] && !archivedIds().has(id);
    const topLevel = (workspace: Workspace) =>
        sessionsFor(engine.state, workspace.path).filter((session) => !archivedIds().has(session.id));
    const workspaceOf = (id: string) => {
        const directory = engine.state.sessions[id]?.directory;
        return directory
            ? workspaces().find((workspace) => normalizeDir(workspace.path) === normalizeDir(directory))
            : undefined;
    };
    const open = (id: string, workspaceId = workspaceOf(id)?.id) => {
        if (workspaceId) selectWorkspace(workspaceId);
        selectSession(id);
    };

    function stepSession(step: 1 | -1) {
        const rows = [...document.querySelectorAll<HTMLElement>("[data-thread-row]")];
        const index = rows.findIndex((row) => row.dataset.threadRow === selectedSession());
        const here = rows.filter((row) => row.dataset.workspaceId === activeWorkspaceId());
        const row = index < 0 && here.length ? stepAround(here, -1, step) : stepAround(rows, index, step);
        if (row?.dataset.threadRow) open(row.dataset.threadRow, row.dataset.workspaceId);
    }

    function stepWorkspace(step: 1 | -1) {
        const list = workspaces();
        const next = stepAround(
            list,
            list.findIndex((workspace) => workspace.id === activeWorkspaceId()),
            step,
        );
        if (!next) return;
        const remembered = lastVisited.get(next.id);
        selectWorkspace(next.id);
        selectSession(remembered && live(remembered) ? remembered : (topLevel(next)[0]?.id ?? null));
    }

    function revisit(direction: 1 | -1) {
        const id = history.step(direction, live);
        if (id) open(id);
    }

    function focusSidebar() {
        showSidebar();
        queueMicrotask(() => {
            const current = document.querySelector<HTMLElement>(
                `[data-thread-row="${CSS.escape(selectedSession() ?? "")}"]`,
            );
            (current ?? document.querySelector<HTMLElement>("[data-sidebar-row]"))?.focus();
        });
    }

    const onSelected = (act: (id: string, workspaceId: string) => void) => () => {
        const id = selectedSession();
        const workspaceId = id ? (workspaceOf(id)?.id ?? activeWorkspaceId()) : undefined;
        if (id && workspaceId) act(id, workspaceId);
    };

    const bindings: [KeybindAction, () => void][] = [
        ["nextSession", () => stepSession(1)],
        ["previousSession", () => stepSession(-1)],
        ["nextWorkspace", () => stepWorkspace(1)],
        ["previousWorkspace", () => stepWorkspace(-1)],
        ["sessionBack", () => revisit(-1)],
        ["sessionForward", () => revisit(1)],
        ["focusSidebar", focusSidebar],
        ["renameSession", onSelected((id) => requestRename(id))],
        ["archiveSession", onSelected((id, workspaceId) => archiveThread(engine, id, workspaceId))],
        ["forkSession", onSelected((id, workspaceId) => void forkThread(engine, id, workspaceId))],
        ["archivedSessions", options.openArchive],
        [
            "addWorkspace",
            () =>
                void pickWorkspace().catch((cause: unknown) =>
                    actionFailed(engine, t("drift.sidebar.addWorkspace"), cause),
                ),
        ],
    ];
    for (const [action, handler] of bindings) onCleanup(onKeybind(action, handler));

    const byDigit = (event: KeyboardEvent) => {
        const digit = sessionDigit(event);
        const workspace = workspaces().find((entry) => entry.id === activeWorkspaceId());
        if (!digit || !workspace || !appKeyFree(event)) return;
        event.preventDefault();
        const session = nthSession(topLevel(workspace), digit);
        if (session) open(session.id, workspace.id);
    };
    window.addEventListener("keydown", byDigit);
    onCleanup(() => window.removeEventListener("keydown", byDigit));
}

/** Arrow keys in the sidebar walk its workspaces and chats as a tree: Left goes up to or folds a workspace, Right opens it. */
export function moveInSidebar(event: KeyboardEvent & { currentTarget: HTMLElement }) {
    const row = event.target instanceof HTMLElement ? event.target.closest<HTMLElement>("[data-sidebar-row]") : null;
    if (!row || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
    const rows = [...event.currentTarget.querySelectorAll<HTMLElement>("[data-sidebar-row]")];
    const index = rows.indexOf(row);
    const target = sidebarTarget(event.key, rows, index, row);
    if (target === undefined) return;
    event.preventDefault();
    target?.focus();
}

const linearMoves: Record<string, (rows: HTMLElement[], index: number) => HTMLElement | undefined> = {
    ArrowDown: (rows, index) => rows[index + 1],
    ArrowUp: (rows, index) => rows[index - 1],
    Home: (rows) => rows[0],
    End: (rows) => rows.at(-1),
};

/** Where a key moves focus: a row, null to stay put having acted, or undefined when the key is not the sidebar's. */
function sidebarTarget(key: string, rows: HTMLElement[], index: number, row: HTMLElement) {
    const linear = linearMoves[key];
    if (linear) return linear(rows, index) ?? null;
    if (key !== "ArrowLeft" && key !== "ArrowRight") return undefined;

    const group = row.closest<HTMLElement>("[data-workspace]");
    if (row.dataset.sidebarRow !== "workspace")
        return key === "ArrowLeft"
            ? (group?.querySelector<HTMLElement>('[data-sidebar-row="workspace"]') ?? null)
            : null;

    const workspace = group?.dataset.workspace;
    if (!workspace) return null;
    const collapsed = workspaceCollapsed(workspace);
    if (key === "ArrowRight" && !collapsed) return rows[index + 1] ?? null;
    if ((key === "ArrowLeft") !== collapsed) toggleWorkspaceCollapsed(workspace);
    return null;
}
