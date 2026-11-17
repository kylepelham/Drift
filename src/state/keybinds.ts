import { persisted } from "./persist";
import { t } from "./i18n";

export type KeybindAction =
    | "palette"
    | "settings"
    | "showShortcuts"
    | "newThread"
    | "searchSessions"
    | "findInSession"
    | "focusComposer"
    | "focusSidebar"
    | "toggleSidebar"
    | "addWorkspace"
    | "nextSession"
    | "previousSession"
    | "sessionBack"
    | "sessionForward"
    | "nextWorkspace"
    | "previousWorkspace"
    | "previousMessage"
    | "nextMessage"
    | "previousPrompt"
    | "nextPrompt"
    | "firstMessage"
    | "lastMessage"
    | "agentPicker"
    | "modelPicker"
    | "thinkingPicker"
    | "attachFiles"
    | "stopTurn"
    | "autoAccept"
    | "toggleUltra"
    | "renameSession"
    | "archiveSession"
    | "forkSession"
    | "toggleReview"
    | "toggleBrowser"
    | "mcpServers"
    | "archivedSessions"
    | "cycleTheme"
    | "zoomIn"
    | "zoomOut"
    | "zoomReset"
    | "nextComputer"
    | "previousComputer";

export type KeybindGroup = "general" | "sessions" | "messages" | "composer" | "session" | "panels" | "view";

/** `global` actions still fire while a dialog is open; the rest belong to the main window. */
export const keybindDefs: { action: KeybindAction; group: KeybindGroup; combo: string | null; global?: boolean }[] = [
    { action: "palette", group: "general", combo: "ctrl+k", global: true },
    { action: "settings", group: "general", combo: "ctrl+," },
    { action: "showShortcuts", group: "general", combo: "f1" },
    { action: "newThread", group: "general", combo: "ctrl+n" },
    { action: "searchSessions", group: "general", combo: "ctrl+shift+f" },
    // Takes over the WebView's own find bar, which cannot see a virtualized transcript.
    { action: "findInSession", group: "general", combo: "ctrl+f" },
    { action: "focusComposer", group: "general", combo: "ctrl+l" },
    { action: "focusSidebar", group: "general", combo: "ctrl+shift+e" },
    { action: "toggleSidebar", group: "general", combo: "ctrl+b" },
    { action: "addWorkspace", group: "general", combo: "ctrl+o" },
    { action: "nextSession", group: "sessions", combo: "ctrl+tab" },
    { action: "previousSession", group: "sessions", combo: "ctrl+shift+tab" },
    { action: "sessionBack", group: "sessions", combo: "alt+arrowleft" },
    { action: "sessionForward", group: "sessions", combo: "alt+arrowright" },
    { action: "nextWorkspace", group: "sessions", combo: "ctrl+alt+arrowdown" },
    { action: "previousWorkspace", group: "sessions", combo: "ctrl+alt+arrowup" },
    // Alt with arrows moves nothing in a text field, so these work while typing.
    { action: "previousMessage", group: "messages", combo: "alt+arrowup" },
    { action: "nextMessage", group: "messages", combo: "alt+arrowdown" },
    { action: "previousPrompt", group: "messages", combo: "alt+shift+arrowup" },
    { action: "nextPrompt", group: "messages", combo: "alt+shift+arrowdown" },
    { action: "firstMessage", group: "messages", combo: "alt+home" },
    { action: "lastMessage", group: "messages", combo: "alt+end" },
    { action: "agentPicker", group: "composer", combo: null },
    { action: "modelPicker", group: "composer", combo: "ctrl+/" },
    { action: "thinkingPicker", group: "composer", combo: "ctrl+." },
    { action: "attachFiles", group: "composer", combo: "ctrl+u" },
    { action: "stopTurn", group: "composer", combo: null },
    { action: "autoAccept", group: "composer", combo: "ctrl+shift+a" },
    { action: "toggleUltra", group: "composer", combo: null },
    { action: "renameSession", group: "session", combo: "f2" },
    { action: "archiveSession", group: "session", combo: null },
    { action: "forkSession", group: "session", combo: null },
    { action: "toggleReview", group: "panels", combo: "ctrl+shift+g" },
    { action: "toggleBrowser", group: "panels", combo: "ctrl+shift+b" },
    { action: "mcpServers", group: "panels", combo: null },
    { action: "archivedSessions", group: "panels", combo: null },
    { action: "cycleTheme", group: "panels", combo: null },
    { action: "zoomIn", group: "view", combo: "ctrl+=", global: true },
    { action: "zoomOut", group: "view", combo: "ctrl+-", global: true },
    { action: "zoomReset", group: "view", combo: "ctrl+0", global: true },
    // Arrows, which AltGr never types with: AltGr is Ctrl+Alt on Windows, and on many layouts brackets need it.
    { action: "nextComputer", group: "view", combo: "ctrl+alt+arrowright" },
    { action: "previousComputer", group: "view", combo: "ctrl+alt+arrowleft" },
];

/** Ctrl+Alt and a digit opens the computer at that place in the computer menu, This PC being 1. Not rebindable. */
export function computerDigit(event: KeyboardEvent) {
    const match = /^ctrl\+alt\+([1-9])$/.exec(eventCombo(event) ?? "");
    return match ? Number(match[1]) : undefined;
}

/** Ctrl and a digit opens the chat at that place in the workspace, 9 being the last. Not rebindable. */
export function sessionDigit(event: KeyboardEvent) {
    const match = /^ctrl\+([1-9])$/.exec(eventCombo(event) ?? "");
    return match ? Number(match[1]) : undefined;
}

const [overrides, setOverrides] = persisted<Partial<Record<KeybindAction, string | null>>>("drift.keybinds", {});

export function comboFor(action: KeybindAction) {
    const current = overrides();
    if (action in current) return current[action] ?? null;
    return keybindDefs.find((def) => def.action === action)?.combo ?? null;
}

export function setCombo(action: KeybindAction, combo: string | null) {
    setOverrides({ ...overrides(), [action]: combo });
}

export function formatCombo(combo: string | null) {
    if (!combo) return t("drift.keybind.unbound");
    return combo
        .split("+")
        .map((part) => part.replace(/^arrow(?=.)/, ""))
        .map((part) => (part.length === 1 ? part.toUpperCase() : part.charAt(0).toUpperCase() + part.slice(1)))
        .join("+");
}

/** A tooltip with the action's shortcut after it, when it has one. */
export function withShortcut(label: string, action: KeybindAction) {
    const combo = comboFor(action);
    return combo ? `${label} (${formatCombo(combo)})` : label;
}

export function eventCombo(event: KeyboardEvent): string | null {
    const key = event.key.toLowerCase();
    if (["control", "shift", "alt", "meta"].includes(key)) return null;
    const normalized = key === "+" ? "=" : key;
    const parts: string[] = [];
    if (event.ctrlKey || event.metaKey) parts.push("ctrl");
    if (event.altKey) parts.push("alt");
    if (event.shiftKey && normalized !== "=") parts.push("shift");
    parts.push(normalized);
    return parts.join("+");
}

/** Whether a dialog covers the window, so only global shortcuts apply. */
function dialogOpen() {
    return !!document.querySelector('[aria-modal="true"]');
}

// A view that takes every key (the browser pane) still lets through what a real browser keeps for itself.
const reservedActions: KeybindAction[] = ["focusComposer", "newThread", "nextSession", "previousSession"];

/** Whether a key press inside a view that takes every key belongs to the app instead. */
export function reservedKey(event: KeyboardEvent) {
    const combo = eventCombo(event);
    return !!combo && reservedActions.some((action) => comboFor(action) === combo);
}

/** Whether a key press is free for the app's shortcuts: unhandled, outside a dialog and outside a view that takes every key. */
export function appKeyFree(event: KeyboardEvent, global = false) {
    if (event.defaultPrevented || event.isComposing) return false;
    const owned = event.target instanceof Element && event.target.closest("[data-own-keys]");
    if (owned && !reservedKey(event)) return false;
    return global || !dialogOpen();
}

const handlers = new Map<KeybindAction, () => void>();

/** Whether an element keeps plain keys for itself: a text field, or a list that moves by letter. */
export function takesKeys(element: Element) {
    return !!element.closest(
        'input, textarea, select, [contenteditable=""], [contenteditable="true"], [role="menu"], [role="listbox"]',
    );
}

/** Registers the one handler for an action and returns its removal. */
export function onKeybind(action: KeybindAction, handler: () => void) {
    handlers.set(action, handler);
    return () => {
        if (handlers.get(action) === handler) handlers.delete(action);
    };
}

export function hasKeybind(action: KeybindAction) {
    return handlers.has(action);
}

export function runKeybind(action: KeybindAction) {
    handlers.get(action)?.();
}

export function initKeybinds() {
    // Bubbling to the window, after any field or menu that handled the key itself.
    window.addEventListener("keydown", (event) => {
        const combo = eventCombo(event);
        if (!combo) return;
        const def = keybindDefs.find((entry) => comboFor(entry.action) === combo);
        if (!def || !appKeyFree(event, def.global)) return;
        // Claimed even when nothing runs, so the WebView's own meaning for the combo never surfaces.
        event.preventDefault();
        handlers.get(def.action)?.();
    });
}
