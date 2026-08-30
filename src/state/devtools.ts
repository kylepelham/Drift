import { shellInvoke } from "../shell"

/**
 * Whether a keyboard event is the DevTools chord. Ctrl+Shift+I mirrors the browser default that
 * WebView2 release builds disable; Alt must stay unpressed so AltGr layouts cannot trigger it.
 */
export function isDevtoolsShortcut(event: Pick<KeyboardEvent, "ctrlKey" | "shiftKey" | "altKey" | "metaKey" | "key">) {
  return event.ctrlKey && event.shiftKey && !event.altKey && !event.metaKey && event.key.toLowerCase() === "i"
}

/**
 * Opens the WebView2 inspector on Ctrl+Shift+I in release builds, where the webview's built-in
 * shortcut is unavailable. Desktop-only: the browser and remote runtimes have their own DevTools.
 * Returns the listener cleanup for `onCleanup`.
 */
export function initDevtoolsShortcut(): () => void {
  const invoke = shellInvoke()
  if (!invoke || typeof window === "undefined") return () => undefined
  const onKeyDown = (event: KeyboardEvent) => {
    if (!isDevtoolsShortcut(event)) return
    event.preventDefault()
    void invoke("open_webview_devtools").catch(() => undefined)
  }
  window.addEventListener("keydown", onKeyDown)
  return () => window.removeEventListener("keydown", onKeyDown)
}
