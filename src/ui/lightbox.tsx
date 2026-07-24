import { createEffect, createSignal, onCleanup, Show } from "solid-js"
import { Portal } from "solid-js/web"
import { t } from "../state/i18n"
import { IconX } from "./icons"

type LightboxImage = { url: string; filename?: string; mime?: string }

const [image, setImage] = createSignal<LightboxImage | null>(null)

export function openLightbox(next: LightboxImage) {
  setImage(next)
}

const minZoom = 0.1
const maxZoom = 8

function dataSize(url: string) {
  if (!url.startsWith("data:")) return null
  const bytes = Math.round(((url.length - url.indexOf(",") - 1) * 3) / 4)
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
  return `${Math.max(1, Math.round(bytes / 1024))} KB`
}

export function Lightbox() {
  const [zoom, setZoom] = createSignal(1)
  const [natural, setNatural] = createSignal<{ w: number; h: number } | null>(null)
  const [fitScale, setFitScale] = createSignal(1)
  let viewport!: HTMLDivElement

  createEffect(() => {
    if (!image()) return
    setZoom(1)
    setNatural(null)
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setImage(null)
    }
    document.addEventListener("keydown", escape)
    onCleanup(() => document.removeEventListener("keydown", escape))
  })

  const clampZoom = (next: number) => Math.min(maxZoom, Math.max(minZoom, next))
  const step = (direction: number) => setZoom(clampZoom(zoom() * (direction > 0 ? 1.25 : 0.8)))
  const percent = () => Math.round(fitScale() * zoom() * 100)
  const width = () => {
    const size = natural()
    return size ? `${size.w * fitScale() * zoom()}px` : undefined
  }

  function measure(img: HTMLImageElement) {
    const rect = viewport.getBoundingClientRect()
    const fit = Math.min((rect.width - 64) / img.naturalWidth, (rect.height - 64) / img.naturalHeight, 1)
    setFitScale(fit > 0 ? fit : 1)
    setNatural({ w: img.naturalWidth, h: img.naturalHeight })
  }

  return (
    <Show when={image()}>
      {(img) => (
        <Portal>
          <div
            class="fixed inset-0 z-50 flex flex-col bg-black/85"
            onPointerDown={(event) => {
              if (!(event.target as HTMLElement).closest("img, button")) setImage(null)
            }}
          >
            <div class="flex items-center gap-3 px-4 py-2.5 text-xs text-white/70 select-none">
              <span class="truncate text-white/90">{img().filename ?? t("drift.lightbox.image")}</span>
              <Show when={img().mime}>
                <span>{img().mime}</span>
              </Show>
              <Show when={natural()}>{(size) => <span>{size().w} × {size().h}</span>}</Show>
              <Show when={dataSize(img().url)}>{(size) => <span>{size()}</span>}</Show>
              <div class="flex-1" />
              <button class="rounded px-2 py-0.5 hover:bg-white/10 hover:text-white" onClick={() => step(-1)}>
                -
              </button>
              <button
                class="w-14 rounded px-2 py-0.5 text-center hover:bg-white/10 hover:text-white"
                title={t("drift.lightbox.resetZoom")}
                onClick={() => setZoom(1)}
              >
                {percent()}%
              </button>
              <button class="rounded px-2 py-0.5 hover:bg-white/10 hover:text-white" onClick={() => step(1)}>
                +
              </button>
              <button
                class="rounded px-2 py-0.5 hover:bg-white/10 hover:text-white"
                title={t("drift.lightbox.actualSize")}
                onClick={() => setZoom(clampZoom(1 / fitScale()))}
              >
                1:1
              </button>
              <button
                title={t("common.close")}
                class="flex size-7 items-center justify-center rounded text-white/70 hover:bg-white/10 hover:text-white"
                onClick={() => setImage(null)}
              >
                <IconX class="size-4" />
              </button>
            </div>
            <div
              ref={(el) => {
                viewport = el
                el.addEventListener(
                  "wheel",
                  (event) => {
                    event.preventDefault()
                    step(event.deltaY < 0 ? 1 : -1)
                  },
                  { passive: false },
                )
              }}
              class="min-h-0 flex-1 overflow-auto"
            >
              <div class="flex min-h-full min-w-fit items-center justify-center p-8">
                <img
                  src={img().url}
                  alt={img().filename ?? t("drift.lightbox.image")}
                  class="select-none"
                  style={{ width: width(), "max-width": natural() ? undefined : "90vw" }}
                  onLoad={(event) => measure(event.currentTarget)}
                  onDblClick={() => setZoom(zoom() === 1 ? clampZoom(1 / fitScale()) : 1)}
                />
              </div>
            </div>
          </div>
        </Portal>
      )}
    </Show>
  )
}
