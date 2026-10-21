import { createSignal } from "solid-js";

/** Sessions whose browser pane the user collapsed; a pane is open otherwise. */
const [collapsed, setCollapsed] = createSignal<ReadonlySet<string>>(new Set());
/** Whether a browser pane is on screen now, for the chat to give way to it on a phone. */
const [showing, setShowing] = createSignal(false);
/** The browser start each session's pane last opened for, so only a new browser opens it again. */
const openedFor = new Map<string, number>();

export { showing as browserPaneShowing, setShowing as setBrowserPaneShowing };

export function browserPaneCollapsed(sessionId: string) {
    return collapsed().has(sessionId);
}

export function collapseBrowserPane(sessionId: string) {
    setCollapsed(new Set([...collapsed(), sessionId]));
}

export function expandBrowserPane(sessionId: string) {
    if (!collapsed().has(sessionId)) return;

    const next = new Set(collapsed());
    next.delete(sessionId);
    setCollapsed(next);
}

/** A browser that started since its session's pane last opened opens it, even if an earlier one was collapsed. */
export function noteBrowserStart(sessionId: string, startedAt: number) {
    if (openedFor.get(sessionId) === startedAt) return;

    openedFor.set(sessionId, startedAt);
    expandBrowserPane(sessionId);
}
