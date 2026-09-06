type DragBox = { id: string; el: HTMLElement; mid: number }

export function dragPointerPressed(pointerId: number, move: Pick<PointerEvent, "pointerId" | "buttons">) {
  return move.pointerId === pointerId && (move.buttons & 1) !== 0
}

export function dragLayoutScale(renderedSize: number, layoutSize: number) {
  const scale = layoutSize > 0 ? renderedSize / layoutSize : 1
  return Number.isFinite(scale) && scale > 0 ? scale : 1
}

export function dragReorderAllowed(event: Pick<PointerEvent, "button" | "isPrimary" | "pointerType">) {
  return event.button === 0 && event.isPrimary && event.pointerType !== "touch"
}

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
  if (!dragReorderAllowed(event)) return () => {}
  const header = event.currentTarget as HTMLElement
  const container = root.parentElement
  const startY = event.clientY
  let pointerY = startY
  let scrollTop = 0
  let headerStyle: { position: string; top: string } | undefined
  let boxes: DragBox[] = []
  let origIndex = 0
  let slot = 0
  let target = 0
  let active = false
  let minDy = 0
  let maxDy = 0
  let rectTop = 0
  let rectBottom = 0
  let scale = 1

  const begin = () => {
    const elements = Array.from(container?.querySelectorAll<HTMLElement>(options.selector) ?? [])
    boxes = elements.map((element) => {
      const rect = element.getBoundingClientRect()
      return { id: options.itemID(element), el: element, mid: rect.top + rect.height / 2 }
    })
    origIndex = boxes.findIndex((box) => box.el === root)
    if (origIndex < 0 || elements.length === 0) return false
    active = true
    header.setPointerCapture(event.pointerId)
    const rect = root.getBoundingClientRect()
    scale = dragLayoutScale(rect.height, root.offsetHeight)
    scrollTop = container?.scrollTop ?? 0
    slot = rect.height + (options.gap ?? 8) * scale
    rectTop = rect.top
    rectBottom = rect.bottom
    minDy = elements[0].getBoundingClientRect().top - rect.top
    maxDy = elements[elements.length - 1].getBoundingClientRect().bottom - rect.bottom
    root.style.position = "relative"
    root.style.zIndex = "10"
    if (header !== root && getComputedStyle(header).position === "sticky") {
      const top = header.getBoundingClientRect().top
      headerStyle = { position: header.style.position, top: header.style.top }
      header.style.position = "relative"
      header.style.top = "0px"
      // Freeze any existing sticky offset; scrolling must move the group only once.
      header.style.top = `${(top - header.getBoundingClientRect().top) / scale}px`
    }
    for (const box of boxes) if (box.el !== root) box.el.style.transition = "transform 150ms ease"
    return true
  }

  const update = () => {
    if (!active) return
    // Cached boxes are viewport pixels; scrollTop and CSS translations are layout pixels.
    const displacement = pointerY - startY + ((container?.scrollTop ?? 0) - scrollTop) * scale
    const dy = Math.min(maxDy, Math.max(minDy, displacement))
    root.style.transform = `translateY(${dy / scale}px)`
    const others = boxes.filter((box) => box.el !== root)
    target = others.filter((box, index) => box.mid < (index < origIndex ? rectTop : rectBottom) + dy).length
    others.forEach((box, index) => {
      const shift = index >= target && index < origIndex ? slot : index >= origIndex && index < target ? -slot : 0
      box.el.style.transform = shift ? `translateY(${shift / scale}px)` : ""
    })
  }

  const onMove = (moveEvent: PointerEvent) => {
    if (moveEvent.pointerId !== event.pointerId) return
    if (!dragPointerPressed(event.pointerId, moveEvent)) return finish(false)
    pointerY = moveEvent.clientY
    if (!active && Math.abs(pointerY - startY) < 5) return
    if (!active && !begin()) return finish(false)
    update()
  }

  const finish = (commit: boolean) => {
    window.removeEventListener("pointermove", onMove)
    window.removeEventListener("pointerup", onUp)
    window.removeEventListener("pointercancel", onCancel)
    window.removeEventListener("blur", onBlur)
    container?.removeEventListener("scroll", update)
    header.removeEventListener("lostpointercapture", onLostCapture)
    if (header.hasPointerCapture(event.pointerId)) header.releasePointerCapture(event.pointerId)
    const wasActive = active
    active = false
    if (!wasActive) return
    for (const box of boxes) {
      box.el.style.transform = ""
      box.el.style.transition = ""
    }
    root.style.position = ""
    root.style.zIndex = ""
    if (headerStyle) {
      header.style.position = headerStyle.position
      header.style.top = headerStyle.top
    }
    if (!commit) return
    const others = boxes.filter((box) => box.el !== root).map((box) => box.id)
    options.move(options.id, others[target] ?? null)
    options.dragged()
  }
  const onUp = (upEvent: PointerEvent) => {
    if (upEvent.pointerId !== event.pointerId) return
    pointerY = upEvent.clientY
    update()
    finish(true)
  }
  const onCancel = (cancelEvent: PointerEvent) => cancelEvent.pointerId === event.pointerId && finish(false)
  const onLostCapture = (captureEvent: PointerEvent) => captureEvent.pointerId === event.pointerId && finish(false)
  const onBlur = () => finish(false)

  window.addEventListener("pointermove", onMove)
  window.addEventListener("pointerup", onUp)
  window.addEventListener("pointercancel", onCancel)
  window.addEventListener("blur", onBlur)
  container?.addEventListener("scroll", update, { passive: true })
  header.addEventListener("lostpointercapture", onLostCapture)
  return () => finish(false)
}
