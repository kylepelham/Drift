import { shellInvoke } from "../shell";

/** Recognizes Ctrl+Shift+I without Alt, avoiding accidental DevTools opening on AltGr layouts. */
export function isDevtoolsShortcut(event: Pick<KeyboardEvent, "ctrlKey" | "shiftKey" | "altKey" | "metaKey" | "key">) {
    return event.ctrlKey && event.shiftKey && !event.altKey && !event.metaKey && event.key.toLowerCase() === "i";
}

/** Enables the inspector shortcut in desktop release builds and returns listener cleanup. */
export function initDevtoolsShortcut(): () => void {
    const invoke = shellInvoke();
    if (!invoke || typeof window === "undefined") return () => undefined;

    const onKeyDown = (event: KeyboardEvent) => {
        if (!isDevtoolsShortcut(event)) return;

        event.preventDefault();
        void invoke("open_webview_devtools").catch(() => undefined);
    };

    window.addEventListener("keydown", onKeyDown);

    return () => window.removeEventListener("keydown", onKeyDown);
}
