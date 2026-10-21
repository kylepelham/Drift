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
