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
  document.addEventListener("keydown", (event) => {
    if (!(event.ctrlKey || event.metaKey) || event.altKey) return
    if (event.key === "=" || event.key === "+") update(zoom() + 0.1)
    else if (event.key === "-") update(zoom() - 0.1)
    else if (event.key === "0") update(1)
    else return
    event.preventDefault()
  })
}
