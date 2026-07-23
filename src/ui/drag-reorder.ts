type DragBox = { id: string; el: HTMLElement; mid: number }

export function dragReorder(
  event: PointerEvent,
  root: HTMLElement,
  options: {
    selector: string
    id: string
    itemID: (element: HTMLElement) => string
    move: (id: string, beforeID: string | null) => void
    dragged: () => void
    gap?: number
  },
) {
  if (event.button !== 0) return
  const header = event.currentTarget as HTMLElement
  const startY = event.clientY
  let boxes: DragBox[] = []
  let origIndex = 0
  let slot = 0
  let target = 0
  let active = false
  let minDy = 0
  let maxDy = 0
  let rectTop = 0
  let rectBottom = 0

  const begin = () => {
    active = true
    header.setPointerCapture(event.pointerId)
    const elements = Array.from(root.parentElement?.querySelectorAll<HTMLElement>(options.selector) ?? [])
    boxes = elements.map((element) => {
      const rect = element.getBoundingClientRect()
      return { id: options.itemID(element), el: element, mid: rect.top + rect.height / 2 }
    })
    origIndex = boxes.findIndex((box) => box.el === root)
    const rect = root.getBoundingClientRect()
    slot = rect.height + (options.gap ?? 8)
    rectTop = rect.top
    rectBottom = rect.bottom
    minDy = elements[0].getBoundingClientRect().top - rect.top
    maxDy = elements[elements.length - 1].getBoundingClientRect().bottom - rect.bottom
    root.style.position = "relative"
    root.style.zIndex = "10"
    for (const box of boxes) if (box.el !== root) box.el.style.transition = "transform 150ms ease"
  }

  const onMove = (moveEvent: PointerEvent) => {
    let dy = moveEvent.clientY - startY
    if (!active && Math.abs(dy) < 5) return
    if (!active) begin()
    dy = Math.min(maxDy, Math.max(minDy, dy))
    root.style.transform = `translateY(${dy}px)`
    const others = boxes.filter((box) => box.el !== root)
    target = others.filter((box, index) => box.mid < (index < origIndex ? rectTop : rectBottom) + dy).length
    others.forEach((box, index) => {
      const shift = index >= target && index < origIndex ? slot : index >= origIndex && index < target ? -slot : 0
      box.el.style.transform = shift ? `translateY(${shift}px)` : ""
    })
  }

  const finish = (commit: boolean) => {
    header.removeEventListener("pointermove", onMove)
    header.removeEventListener("pointerup", onUp)
    header.removeEventListener("pointercancel", onCancel)
    if (!active) return
    for (const box of boxes) {
      box.el.style.transform = ""
      box.el.style.transition = ""
    }
    root.style.position = ""
    root.style.zIndex = ""
    if (!commit) return
    const others = boxes.filter((box) => box.el !== root).map((box) => box.id)
    options.move(options.id, others[target] ?? null)
    options.dragged()
  }
  const onUp = () => finish(true)
  const onCancel = () => finish(false)

  header.addEventListener("pointermove", onMove)
  header.addEventListener("pointerup", onUp)
  header.addEventListener("pointercancel", onCancel)
}
