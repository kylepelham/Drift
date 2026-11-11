/** Accesses the injected desktop bridge; missing browser-runtime capabilities return undefined. */

import { isRemoteRuntime } from "./runtime";

export type ShellInvoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

export type ShellWindow = {
    minimize(): void;
    toggleMaximize(): void;
    close(): void;
    isMaximized(): Promise<boolean>;
    onResized(handler: () => void): Promise<() => void>;
};

export type ShellWebview = {
    setZoom(factor: number): Promise<void>;
};

export type ShellEvents = {
    listen<T = unknown>(name: string, handler: (event: { payload: T }) => void): Promise<() => void>;
};

type TauriGlobal = {
    core?: { invoke?: ShellInvoke };
    window?: { getCurrentWindow?: () => ShellWindow };
    webview?: { getCurrentWebview?: () => ShellWebview };
    event?: ShellEvents;
};

function tauri() {
    const global = (globalThis as { __TAURI__?: TauriGlobal }).__TAURI__;
    // Another computer's Drift, shown in this window through the desktop's proxy, drives the window and nothing else
    // of this PC: its commands, engine and events are that computer's.
    if (global && isRemoteRuntime()) return { window: global.window, webview: global.webview };

    return global;
}

/** True when running inside the desktop shell rather than a browser. */
export function isDesktopShell() {
    return Boolean(shellInvoke());
}

/** Invokes a Rust command, or `undefined` when there is no desktop backend. */
export function shellInvoke(): ShellInvoke | undefined {
    return tauri()?.core?.invoke;
}

/** Opens a page in the user's browser: the desktop opener, else a new tab. */
export function openExternal(url: string) {
    const invoke = shellInvoke();
    if (invoke) {
        void invoke("plugin:opener|open_url", { url }).catch(() => {});
        return;
    }

    window.open(url, "_blank");
}

export function shellWindow(): ShellWindow | undefined {
    return tauri()?.window?.getCurrentWindow?.();
}

export function shellWebview(): ShellWebview | undefined {
    return tauri()?.webview?.getCurrentWebview?.();
}

export function shellEvents(): ShellEvents | undefined {
    return tauri()?.event;
}
