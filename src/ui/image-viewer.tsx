import { createSignal, createUniqueId, onCleanup, onMount, Show, type JSX } from "solid-js"
import { t } from "../state/i18n"
import { IconRestore } from "./icons"
import { containImage, fitImage, imageWheelScale, maxImageScale, zoomImageAt, type ImagePoint, type ImageSize, type ImageTransform } from "./image-transform"

type ImageViewerProps = {
  src: string
  alt: string
  onError?: () => void
  onBackgroundClick?: () => void
  toolbarStart?: JSX.Element
  toolbarEnd?: JSX.Element
}

export function ImageViewer(props: ImageViewerProps) {
  return <Show when={props.src} keyed>{(src) => <ImageCanvas {...props} src={src} />}</Show>
}

function ImageCanvas(props: ImageViewerProps) {
  const compactToolbar = "toolbarStart" in props || "toolbarEnd" in props
  let viewport!: HTMLDivElement
  let img!: HTMLImageElement
  const hintID = createUniqueId()
  const [natural, setNatural] = createSignal<ImageSize>()
  const [view, setView] = createSignal<ImageTransform>({ x: 0, y: 0, scale: 1 })
  const [dragging, setDragging] = createSignal(false)
  const [failed, setFailed] = createSignal(false)
  let size: ImageSize = { width: 0, height: 0 }
  let fitted = true
  const pointers = new Map<number, ImagePoint>()
  let start: ImagePoint | undefined
  let background = false
  let moved = false
  const center = () => ({ x: size.width / 2, y: size.height / 2 })
  const minimum = () => Math.min(0.01, fitImage(natural()!, size).scale / 10)
  const bounded = (scale: number) => Math.min(maxImageScale, Math.max(minimum(), scale))

  function reset() {
    if (!natural()) return
    fitted = true
    setView(fitImage(natural()!, size))
  }

  function zoom(scale: number, anchor = center()) {
    if (!natural() || !Number.isFinite(scale)) return
    fitted = false
    setView((previous) => containImage(zoomImageAt(previous, bounded(scale), anchor), natural()!, size))
  }

  function pan(x: number, y: number) {
    if (!natural()) return
    fitted = false
    setView((previous) => containImage({ ...previous, x: previous.x + x, y: previous.y + y }, natural()!, size))
  }

  function point(event: { clientX: number; clientY: number }): ImagePoint {
    const rect = viewport.getBoundingClientRect()
    // CSS zoom in browser development changes client coordinates, not layout pixels.
    return { x: (event.clientX - rect.left) * size.width / rect.width, y: (event.clientY - rect.top) * size.height / rect.height }
  }

  function measure() {
    const previous = size
    const style = getComputedStyle(viewport)
    size = { width: Number.parseFloat(style.width), height: Number.parseFloat(style.height) }
    if (!natural() || !size.width || !size.height) return
    if (fitted) reset()
    else setView((current) => containImage({ ...current, x: current.x + (size.width - previous.width) / 2, y: current.y + (size.height - previous.height) / 2 }, natural()!, size))
    pointers.clear()
    setDragging(false)
  }

  function loaded() {
    if (!img.naturalWidth || !img.naturalHeight) return
    setNatural({ width: img.naturalWidth, height: img.naturalHeight })
    measure()
  }

  function endPointer(event: PointerEvent) {
    if (!pointers.has(event.pointerId)) return
    const dismiss = event.type === "pointerup" && pointers.size === 1 && background && !moved
    pointers.delete(event.pointerId)
    if (viewport.hasPointerCapture(event.pointerId)) viewport.releasePointerCapture(event.pointerId)
    setDragging(pointers.size > 0)
    if (dismiss) props.onBackgroundClick?.()
  }

  onMount(() => {
    measure()
    if (img.complete) loaded()
    const observer = new ResizeObserver(measure)
    observer.observe(viewport)
    const wheel = (event: WheelEvent) => {
      event.preventDefault()
      event.stopPropagation()
      zoom(view().scale * imageWheelScale(event.deltaY, event.deltaMode, size.height), point(event))
    }
    viewport.addEventListener("wheel", wheel, { passive: false })
    const blur = () => {
      for (const id of pointers.keys()) if (viewport.hasPointerCapture(id)) viewport.releasePointerCapture(id)
      pointers.clear()
      setDragging(false)
    }
    window.addEventListener("blur", blur)
    onCleanup(() => {
      observer.disconnect()
      viewport.removeEventListener("wheel", wheel)
      window.removeEventListener("blur", blur)
      blur()
    })
  })

  return (
    <div class="flex min-h-0 min-w-0 flex-1 flex-col text-ink" data-image-viewer onKeyDown={(event) => {
      if (event.ctrlKey || event.metaKey || event.altKey || !natural()) return
      switch (event.key) {
        case "+": case "=": zoom(view().scale * 1.25); break
        case "-": case "_": zoom(view().scale / 1.25); break
        case "0": case "Home": reset(); break
        case "1": zoom(1); break
        case "ArrowLeft": pan(event.shiftKey ? 160 : 40, 0); break
        case "ArrowRight": pan(event.shiftKey ? -160 : -40, 0); break
        case "ArrowUp": pan(0, event.shiftKey ? 160 : 40); break
        case "ArrowDown": pan(0, event.shiftKey ? -160 : -40); break
        default: return
      }
      event.preventDefault()
      event.stopPropagation()
    }}>
      <div class="flex min-w-0 shrink-0 items-center gap-1 border-b border-edge bg-surface px-2 py-1.5 text-xs"
        classList={{ "flex-wrap justify-center": !compactToolbar, "flex-nowrap whitespace-nowrap": compactToolbar }} data-image-toolbar>
        {props.toolbarStart}
        <button type="button" class="flex h-8 min-w-8 shrink-0 items-center justify-center rounded hover:bg-raised disabled:opacity-40" disabled={!natural()} aria-label={t("drift.preview.zoomOut")} onClick={() => zoom(view().scale / 1.25)}>-</button>
        <span class="min-w-16 shrink-0 text-center tabular-nums" data-image-zoom>{natural() ? `${Number((view().scale * 100).toFixed(2))}%` : "..."}</span>
        <button type="button" class="flex h-8 min-w-8 shrink-0 items-center justify-center rounded hover:bg-raised disabled:opacity-40" disabled={!natural()} aria-label={t("drift.preview.zoomIn")} onClick={() => zoom(view().scale * 1.25)}>+</button>
        <button type="button" class="flex h-8 min-w-8 shrink-0 items-center justify-center rounded px-2 hover:bg-raised disabled:opacity-40" disabled={!natural()} aria-label={t("drift.lightbox.resetZoom")} title={t("drift.lightbox.resetZoom")} onClick={reset}>
          <Show when={compactToolbar} fallback={t("drift.lightbox.resetZoom")}>
            <IconRestore class="size-4 lg:hidden" />
            <span class="hidden lg:inline">{t("drift.lightbox.resetZoom")}</span>
          </Show>
        </button>
        <button type="button" class="h-8 shrink-0 rounded px-2 hover:bg-raised disabled:opacity-40" disabled={!natural()} aria-label={t("drift.lightbox.actualSize")} title={t("drift.lightbox.actualSize")} onClick={() => zoom(1)}>1:1</button>
        <Show when={natural()}>{(dimensions) => <span class="px-2 text-ink-faint tabular-nums" classList={{ "hidden lg:inline": compactToolbar }}>{dimensions().width} × {dimensions().height}</span>}</Show>
        {props.toolbarEnd}
      </div>
      <div ref={viewport} role="region" tabIndex={0} aria-label={props.alt} aria-describedby={hintID}
        data-image-viewport class="relative min-h-0 min-w-0 flex-1 touch-none overflow-hidden overscroll-contain bg-black/30 outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-accent"
        style={{ cursor: natural() ? dragging() ? "grabbing" : "grab" : "default" }}
        onPointerDown={(event) => {
          if (!natural() || (event.button !== 0 && event.button !== 1)) return
          event.preventDefault()
          viewport.focus({ preventScroll: true })
          const location = point(event)
          if (!pointers.size) {
            start = location
            moved = false
            background = event.target === viewport && event.button === 0
          } else moved = true
          pointers.set(event.pointerId, location)
          viewport.setPointerCapture(event.pointerId)
          setDragging(true)
        }}
        onPointerMove={(event) => {
          const previous = pointers.get(event.pointerId)
          if (!previous || !natural()) return
          if (event.pointerType === "mouse" && !(event.buttons & 5)) {
            moved = true
            endPointer(event)
            return
          }
          const location = point(event)
          if (start && Math.hypot(location.x - start.x, location.y - start.y) > 3) moved = true
          const before = [...pointers.values()]
          pointers.set(event.pointerId, location)
          if (pointers.size === 1) pan(location.x - previous.x, location.y - previous.y)
          else {
            const after = [...pointers.values()]
            const oldCenter = { x: (before[0].x + before[1].x) / 2, y: (before[0].y + before[1].y) / 2 }
            const newCenter = { x: (after[0].x + after[1].x) / 2, y: (after[0].y + after[1].y) / 2 }
            const oldDistance = Math.hypot(before[0].x - before[1].x, before[0].y - before[1].y)
            const newDistance = Math.hypot(after[0].x - after[1].x, after[0].y - after[1].y)
            if (oldDistance < 1) return
            fitted = false
            setView((current) => {
              const next = zoomImageAt(current, bounded(current.scale * newDistance / oldDistance), oldCenter)
              return containImage({ ...next, x: next.x + newCenter.x - oldCenter.x, y: next.y + newCenter.y - oldCenter.y }, natural()!, size)
            })
          }
        }}
        onPointerUp={endPointer} onPointerCancel={endPointer} onLostPointerCapture={endPointer}
        onDblClick={(event) => {
          if (!natural() || moved || background) return
          // Pointer capture retargets dblclick to the viewport even when both presses hit the image.
          if (view().scale > fitImage(natural()!, size).scale * 1.01) reset()
          else zoom(Math.max(1, view().scale * 2), point(event))
        }}>
        <img ref={img} src={props.src} alt={props.alt} draggable={false}
          class="absolute left-0 top-0 max-w-none select-none" style={{ width: natural() ? `${natural()!.width}px` : undefined, height: natural() ? `${natural()!.height}px` : undefined, visibility: natural() ? "visible" : "hidden", "transform-origin": "0 0", transform: `translate(${view().x}px, ${view().y}px) scale(${view().scale})` }}
          onLoad={loaded} onError={() => { setFailed(true); props.onError?.() }} onDragStart={(event) => event.preventDefault()} />
        <Show when={!natural()}><p role={failed() ? "alert" : "status"} class="absolute inset-0 grid place-items-center p-6 text-center text-sm text-ink-muted">{t(failed() ? "drift.preview.mediaError" : "drift.preview.loading")}</p></Show>
      </div>
      <p id={hintID} class="shrink-0 border-t border-edge bg-surface px-3 py-1.5 text-center text-[0.65rem] text-ink-muted">{t("drift.lightbox.controls")}</p>
    </div>
  )
}
