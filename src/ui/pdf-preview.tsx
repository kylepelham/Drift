import { createEffect, createSignal, onCleanup, onMount, Show, untrack } from "solid-js"
import { getDocument, GlobalWorkerOptions, type PDFDocumentProxy, type RenderTask } from "pdfjs-dist"
import workerSrc from "pdfjs-dist/build/pdf.worker.mjs?url"
import { t } from "../state/i18n"

GlobalWorkerOptions.workerSrc = workerSrc

const minZoom = 0.25
const maxZoom = 4
const maxCanvasPixels = 8_000_000
const maxCanvasSide = 8192

export function PdfPreview(props: { data: Uint8Array; initialPage?: number }) {
  const [pdf, setPdf] = createSignal<PDFDocumentProxy>()
  const [pageNumber, setPageNumber] = createSignal(1)
  const [zoom, setZoom] = createSignal(1)
  const [width, setWidth] = createSignal(0)
  const [busy, setBusy] = createSignal(true)
  const [error, setError] = createSignal<string>()
  let container!: HTMLDivElement
  let pageHost!: HTMLDivElement
  let cancelRender: (() => void) | undefined
  let destroyLoading: (() => void) | undefined

  const pages = () => pdf()?.numPages ?? 0
  const pageLabel = () => t("drift.preview.page", { page: pageNumber(), pages: pages() })
  const changePage = (value: number) => {
    if (Number.isFinite(value)) setPageNumber(Math.max(1, Math.min(pages(), Math.trunc(value))))
  }
  const changeZoom = (value: number) => setZoom(Math.max(minZoom, Math.min(maxZoom, value)))

  onMount(() => {
    const measure = () => setWidth(Math.max(0, container.clientWidth - 32))
    measure()
    if (typeof ResizeObserver !== "undefined") {
      const observer = new ResizeObserver(measure)
      observer.observe(container)
      onCleanup(() => observer.disconnect())
    } else {
      window.addEventListener("resize", measure)
      onCleanup(() => window.removeEventListener("resize", measure))
    }
  })

  createEffect(() => {
    const data = props.data
    const initialPage = untrack(() => props.initialPage)
    let active = true
    let loading: ReturnType<typeof getDocument> | undefined
    let destroyed = false
    const destroy = () => {
      if (destroyed) return
      destroyed = true
      cancelRender?.()
      // Destruction can reject while worker initialization is still in flight.
      if (loading) void loading.destroy().catch(() => {})
    }
    destroyLoading = destroy
    onCleanup(() => {
      active = false
      destroy()
    })
    setPdf(undefined)
    setError(undefined)
    setBusy(true)
    setZoom(1)
    setPageNumber(1)
    void (async () => {
      try {
        loading = getDocument({
          // PDF.js transfers the buffer to its worker; the parent's bytes must survive.
          data: new Uint8Array(data),
          useWorkerFetch: false,
          useWasm: false,
          useSystemFonts: true,
          enableXfa: false,
          // No CMap/font/WASM URLs: the main-thread asset factory rejects before fetching.
          canvasMaxAreaInBytes: maxCanvasPixels * 4,
        })
        const document = await loading.promise
        if (!active) return
        if (document.numPages < 1) throw new Error("PDF has no pages")
        setPageNumber(Number.isFinite(initialPage) ? Math.max(1, Math.min(document.numPages, Math.trunc(initialPage!))) : 1)
        setPdf(document)
      } catch (reason) {
        if (active) {
          setError(reason instanceof Error && reason.name === "PasswordException" ? "drift.preview.pdfPassword" : "drift.preview.error")
          setBusy(false)
        }
        destroy()
      }
    })()
  })

  createEffect(() => {
    const document = pdf()
    const number = pageNumber()
    const availableWidth = width()
    const magnification = zoom()
    if (!document || availableWidth <= 0) return
    let active = true
    let settled = false
    let task: RenderTask | undefined
    let canvas: HTMLCanvasElement | undefined
    const releaseCanvas = () => {
      if (canvas) canvas.width = canvas.height = 0
    }
    const cancel = () => {
      if (!active) return
      active = false
      task?.cancel()
      canvas?.remove()
      if (settled) releaseCanvas()
    }
    cancelRender = cancel
    onCleanup(cancel)
    setBusy(true)
    setError(undefined)
    container.scrollTop = container.scrollLeft = 0
    void (async () => {
      try {
        const page = await document.getPage(number)
        if (!active) return
        const base = page.getViewport({ scale: 1 })
        if (!Number.isFinite(base.width) || !Number.isFinite(base.height) || base.width <= 0 || base.height <= 0) {
          throw new Error("Invalid PDF page dimensions")
        }
        const viewport = page.getViewport({ scale: availableWidth / base.width * magnification })
        if (!Number.isFinite(viewport.width) || !Number.isFinite(viewport.height) || viewport.width <= 0 || viewport.height <= 0) {
          throw new Error("Invalid PDF viewport")
        }
        const outputScale = Math.min(
          window.devicePixelRatio || 1,
          Math.sqrt(maxCanvasPixels / viewport.width / viewport.height),
          maxCanvasSide / viewport.width,
          maxCanvasSide / viewport.height,
        )
        // Never reuse a canvas whose cancelled render promise may still be settling.
        canvas = window.document.createElement("canvas")
        canvas.width = Math.max(1, Math.floor(viewport.width * outputScale))
        canvas.height = Math.max(1, Math.floor(viewport.height * outputScale))
        canvas.style.width = `${viewport.width}px`
        canvas.style.height = `${viewport.height}px`
        canvas.style.display = "block"
        canvas.style.background = "white"
        const context = canvas.getContext("2d")
        if (!context) throw new Error("Canvas is unavailable")
        task = page.render({
          canvas,
          canvasContext: context,
          viewport,
          transform: [outputScale, 0, 0, outputScale, 0, 0],
          background: "white",
        })
        await task.promise
        if (!active) return
        pageHost.replaceChildren(canvas)
        setBusy(false)
      } catch {
        if (!active) return
        setError("drift.preview.error")
        setBusy(false)
        destroyLoading?.()
        setPdf(undefined)
      } finally {
        settled = true
        if (!active) releaseCanvas()
      }
    })()
  })

  const buttonClass = "flex size-10 shrink-0 items-center justify-center rounded border border-edge text-ink-muted hover:bg-raised hover:text-ink disabled:opacity-40 focus-visible:outline-2 focus-visible:outline-accent"

  return (
    <div class="flex h-full min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
      <Show when={pdf()}>
        <div class="flex shrink-0 flex-wrap items-center justify-center gap-2 border-b border-edge p-2 text-sm text-ink">
          <div class="flex items-center gap-2">
            <button type="button" class={buttonClass} aria-label={t("drift.preview.previous")} title={t("drift.preview.previous")} disabled={pageNumber() <= 1} onClick={() => changePage(pageNumber() - 1)}>
              <span aria-hidden="true">&lt;</span>
            </button>
            <input
              type="number"
              inputMode="numeric"
              min={1}
              max={pages()}
              step={1}
              value={pageNumber()}
              aria-label={pageLabel()}
              class="h-10 w-16 rounded border border-edge bg-surface px-2 text-center tabular-nums focus-visible:outline-2 focus-visible:outline-accent"
              onChange={(event) => {
                changePage(event.currentTarget.valueAsNumber)
                event.currentTarget.value = String(pageNumber())
              }}
            />
            <button type="button" class={buttonClass} aria-label={t("drift.preview.next")} title={t("drift.preview.next")} disabled={pageNumber() >= pages()} onClick={() => changePage(pageNumber() + 1)}>
              <span aria-hidden="true">&gt;</span>
            </button>
          </div>
          <span class="text-xs text-ink-muted" role="status">{pageLabel()}</span>
          <div class="flex items-center gap-2">
            <button type="button" class={buttonClass} aria-label={t("drift.preview.zoomOut")} title={t("drift.preview.zoomOut")} disabled={zoom() <= minZoom} onClick={() => changeZoom(zoom() / 1.25)}>
              <span aria-hidden="true">-</span>
            </button>
            <span class="w-12 text-center text-xs tabular-nums">{Math.round(zoom() * 100)}%</span>
            <button type="button" class={buttonClass} aria-label={t("drift.preview.zoomIn")} title={t("drift.preview.zoomIn")} disabled={zoom() >= maxZoom} onClick={() => changeZoom(zoom() * 1.25)}>
              <span aria-hidden="true">+</span>
            </button>
          </div>
        </div>
      </Show>
      <Show when={busy()}><p role="status" class="shrink-0 p-3 text-center text-sm text-ink-muted">{t("drift.preview.loading")}</p></Show>
      <Show when={error()}>{(key) => <p role="alert" class="p-4 text-center text-sm text-ink-muted">{t(key())}</p>}</Show>
      <div ref={container} class="min-h-0 min-w-0 flex-1 overflow-auto overscroll-contain" style={{ "scrollbar-gutter": "stable" }} tabIndex={0} role="region" aria-label={t("drift.preview.type.pdf")} aria-busy={busy()}>
        <div class="flex w-max min-w-full justify-center p-4">
          <div ref={pageHost} role="img" aria-label={pageLabel()} class="shrink-0" />
        </div>
      </div>
    </div>
  )
}
