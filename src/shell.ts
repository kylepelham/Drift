/**
 * Access to the Tauri bridge the desktop shell injects on `globalThis`.
 *
 * Drift also runs as a plain web app, where none of this exists, so every accessor returns
 * `undefined` rather than throwing and callers are expected to fall back. Declaring the bridge once
 * here replaces six separately-written inline casts that each described a different subset of it.
 */

export type ShellInvoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>

export type ShellWindow = {
  minimize(): void
  toggleMaximize(): void
  close(): void
  isMaximized(): Promise<boolean>
  onResized(handler: () => void): Promise<() => void>
}

export type ShellWebview = {
  setZoom(factor: number): Promise<void>
}

export type ShellEvents = {
  listen(name: string, handler: () => void): Promise<() => void>
}

type TauriGlobal = {
  core?: { invoke?: ShellInvoke }
  window?: { getCurrentWindow?: () => ShellWindow }
  webview?: { getCurrentWebview?: () => ShellWebview }
  event?: ShellEvents
}

function tauri() {
  return (globalThis as { __TAURI__?: TauriGlobal }).__TAURI__
}

/** True when running inside the desktop shell rather than a browser. */
export function isDesktopShell() {
  return Boolean(tauri())
}

/** Invokes a Rust command, or `undefined` when there is no desktop backend. */
export function shellInvoke(): ShellInvoke | undefined {
  return tauri()?.core?.invoke
}

export function shellWindow(): ShellWindow | undefined {
  return tauri()?.window?.getCurrentWindow?.()
}

export function shellWebview(): ShellWebview | undefined {
  return tauri()?.webview?.getCurrentWebview?.()
}

export function shellEvents(): ShellEvents | undefined {
  return tauri()?.event
}

/** The shell draws its own titlebar, so the app renders a custom one only when it is present. */
export function hasNativeWindow() {
  return Boolean(tauri()?.window)
}
