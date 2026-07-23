import { onKeybind } from "./keybinds"
import { persisted } from "./persist"

const [zoom, setZoom] = persisted<number>("drift.zoom", 1)
let applyRequest = 0

const clamp = (value: number) => Math.min(2, Math.max(0.5, Math.round(value * 10) / 10))

function cssZoom(value: number) {
  ;(document.documentElement.style as CSSStyleDeclaration & { zoom: string }).zoom = value === 1 ? "" : String(value)
}

function apply(value: number) {
  const current = ++applyRequest
  const getCurrentWebview = (
    globalThis as {
      __TAURI__?: { webview?: { getCurrentWebview?: () => { setZoom: (factor: number) => Promise<void> } } }
    }
  ).__TAURI__?.webview?.getCurrentWebview
  if (!getCurrentWebview) return cssZoom(value)
  cssZoom(1)
  void getCurrentWebview()
    .setZoom(value)
    .catch(() => current === applyRequest && cssZoom(value))
}

function update(next: number) {
  const value = clamp(next)
  setZoom(value)
  apply(value)
}

export function initZoom() {
  apply(zoom())
  onKeybind("zoomIn", () => update(zoom() + 0.1))
  onKeybind("zoomOut", () => update(zoom() - 0.1))
  onKeybind("zoomReset", () => update(1))
}

type PositionMetrics = { scale: number; viewportWidth: number; viewportHeight: number }

export function fixedMenuPosition(
  clientX: number,
  clientY: number,
  menuWidth: number,
  menuHeight: number,
  metrics?: PositionMetrics,
) {
  zoom()
  const scale = (metrics?.scale ?? Number(getComputedStyle(document.documentElement).zoom)) || 1
  const viewportWidth = (metrics?.viewportWidth ?? window.innerWidth) / scale
  const viewportHeight = (metrics?.viewportHeight ?? window.innerHeight) / scale
  const margin = 8
  return {
    left: Math.max(margin, Math.min(clientX / scale, viewportWidth - menuWidth - margin)),
    top: Math.max(margin, Math.min(clientY / scale, viewportHeight - menuHeight - margin)),
    viewportHeight,
  }
}
