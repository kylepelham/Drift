import { onKeybind } from "./keybinds"
import { persisted } from "./persist"

const [zoom, setZoom] = persisted<number>("drift.zoom", 1)

const clamp = (value: number) => Math.min(2, Math.max(0.5, Math.round(value * 10) / 10))

function apply(value: number) {
  ;(document.documentElement.style as CSSStyleDeclaration & { zoom: string }).zoom = value === 1 ? "" : String(value)
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
