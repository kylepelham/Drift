import { t } from "../state/i18n";

import type { BrowserKind } from "../engine/native/client";

/** An engine's name as people say it; only IE mode is a description rather than a product. */
export function engineLabel(kind: BrowserKind) {
    const names: Record<BrowserKind, string> = {
        chrome: "Chrome",
        edge: "Edge",
        firefox: "Firefox",
        camoufox: "Camoufox",
        ie: t("drift.settings.browser.engine.ie"),
    };

    return names[kind];
}

/** The DevTools Protocol's modifier bits for the keys held during an event. */
export function modifierBits(event: { altKey: boolean; ctrlKey: boolean; metaKey: boolean; shiftKey: boolean }) {
    let bits = 0;
    if (event.altKey) bits |= 1;
    if (event.ctrlKey) bits |= 2;
    if (event.metaKey) bits |= 4;
    if (event.shiftKey) bits |= 8;

    return bits;
}

/** A point on the shown picture as the page's CSS pixels. */
export function pagePoint(
    client: { clientX: number; clientY: number },
    shown: { left: number; top: number; width: number; height: number },
    page: { width: number; height: number },
) {
    const x = ((client.clientX - shown.left) / Math.max(shown.width, 1)) * page.width;
    const y = ((client.clientY - shown.top) / Math.max(shown.height, 1)) * page.height;

    return { x: Math.round(x), y: Math.round(y) };
}

const buttons = ["left", "middle", "right"] as const;

export function buttonName(button: number) {
    return buttons[button] ?? "left";
}

/** Paste reaches the page as text, so the shortcut itself is left to the browser's paste event. */
export function isPasteShortcut(event: { key: string; ctrlKey: boolean; metaKey: boolean }) {
    return (event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "v";
}

/** A short name for a tab: its title, else its host, else the address. */
export function tabLabel(page: { title: string; url: string }) {
    if (page.title.trim()) return page.title;

    try {
        const url = new URL(page.url);
        return url.host || page.url;
    } catch {
        return page.url;
    }
}

type PaneFile = { name: string; mime: string; data: string };

/** The most a paste or drop sends to the browser at once, so one large file never stalls the pane's socket. */
const paneFileLimit = 25 * 1024 * 1024;

/** Files the user pasted or dropped, as the engine takes them; any past the limit are named instead of sent. */
export async function paneFiles(list: Iterable<File>) {
    const files: PaneFile[] = [];
    const skipped: string[] = [];
    let total = 0;
    for (const file of list) {
        total += file.size;
        if (total > paneFileLimit) {
            skipped.push(file.name);
            continue;
        }
        files.push({ name: file.name, mime: file.type, data: await base64(file) });
    }

    return { files, skipped };
}

function base64(file: Blob) {
    return new Promise<string>((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = () => resolve(String(reader.result).replace(/^data:[^,]*,/, ""));
        reader.onerror = () => reject(reader.error ?? new Error("unreadable"));
        reader.readAsDataURL(file);
    });
}

/** Keys the hidden text box keeps for itself: an input method composing, a dead key, and Ctrl+V, whose paste event carries what is pasted. */
export function composesOrPastes(event: KeyboardEvent) {
    return (
        event.isComposing ||
        event.key === "Process" ||
        event.key === "Dead" ||
        event.keyCode === 229 ||
        isPasteShortcut(event)
    );
}
