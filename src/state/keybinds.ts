import { persisted } from "./persist";
import { t } from "./i18n";

export type KeybindAction =
    | "palette"
    | "newThread"
    | "findInSession"
    | "autoAccept"
    | "zoomIn"
    | "zoomOut"
    | "zoomReset"
    | "nextComputer"
    | "previousComputer";

export const keybindDefs: { action: KeybindAction; combo: string | null }[] = [
    { action: "palette", combo: "ctrl+k" },
    { action: "newThread", combo: "ctrl+n" },
    // Takes over the WebView's own find bar, which cannot see a virtualized transcript.
    { action: "findInSession", combo: "ctrl+f" },
    { action: "autoAccept", combo: "ctrl+shift+a" },
    { action: "zoomIn", combo: "ctrl+=" },
    { action: "zoomOut", combo: "ctrl+-" },
    { action: "zoomReset", combo: "ctrl+0" },
    // Arrows, which AltGr never types with: AltGr is Ctrl+Alt on Windows, and on many layouts brackets need it.
    { action: "nextComputer", combo: "ctrl+alt+arrowright" },
    { action: "previousComputer", combo: "ctrl+alt+arrowleft" },
];

/** Ctrl+Alt and a digit opens the computer at that place in the computer menu, This PC being 1. Not rebindable. */
export function computerDigit(event: KeyboardEvent) {
    const match = /^ctrl\+alt\+([1-9])$/.exec(eventCombo(event) ?? "");
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

const handlers = new Map<KeybindAction, () => void>();

export function onKeybind(action: KeybindAction, handler: () => void) {
    handlers.set(action, handler);
}

export function initKeybinds() {
    document.addEventListener("keydown", (event) => {
        const combo = eventCombo(event);
        if (!combo) return;
        for (const def of keybindDefs) {
            if (comboFor(def.action) !== combo) continue;
            event.preventDefault();
            handlers.get(def.action)?.();
            return;
        }
    });
}
