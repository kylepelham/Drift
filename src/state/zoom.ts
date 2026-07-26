import { shellWebview } from "../shell"
import { onKeybind } from "./keybinds"
import { createLatestOnly } from "./latest"
import { persisted } from "./persist"

const [zoom, setZoom] = persisted<number>("drift.zoom", 1)
const zoomApply = createLatestOnly()

const minZoom = 0.5
const maxZoom = 2
const zoomStep = 0.1
// Zoom is kept to one decimal so repeated steps do not accumulate floating point drift.
const zoomPrecision = 10

const clamp = (value: number) =>
  Math.min(maxZoom, Math.max(minZoom, Math.round(value * zoomPrecision) / zoomPrecision))

function cssZoom(value: number) {
  ;(document.documentElement.style as CSSStyleDeclaration & { zoom: string }).zoom = value === 1 ? "" : String(value)
}

function apply(value: number) {
  const token = zoomApply.begin()
  const webview = shellWebview()
  // In a browser there is no native zoom, so CSS is the only mechanism.
  if (!webview) return cssZoom(value)
  // Native zoom is preferred because it scales the whole webview, so the CSS zoom is reset first
  // and only reinstated as a fallback if the native call fails and no newer zoom has started.
  cssZoom(1)
  void webview.setZoom(value).catch(() => zoomApply.isCurrent(token) && cssZoom(value))
}

function update(next: number) {
  const value = clamp(next)
  setZoom(value)
  apply(value)
}

export function initZoom() {
  apply(zoom())
  onKeybind("zoomIn", () => update(zoom() + zoomStep))
  onKeybind("zoomOut", () => update(zoom() - zoomStep))
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
