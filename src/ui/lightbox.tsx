import { createSignal, onCleanup, onMount, Show } from "solid-js"
import { Portal } from "solid-js/web"
import { t } from "../state/i18n"
import { IconX } from "./icons"
import { activateModal, closeOnBackdropPointerDown } from "./modal"

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
  return (
    <Show when={image()} keyed>
      {(item) => (
        <Portal>
          <LightboxDialog image={item} onClose={() => setImage(null)} />
        </Portal>
      )}
    </Show>
  )
}

function LightboxDialog(props: { image: LightboxImage; onClose: () => void }) {
  const [zoom, setZoom] = createSignal(1)
  const [natural, setNatural] = createSignal<{ w: number; h: number } | null>(null)
  const [fitScale, setFitScale] = createSignal(1)
  let dialog!: HTMLDivElement
  let viewport!: HTMLDivElement
  onMount(() => onCleanup(activateModal(dialog, props.onClose)))

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
    <div
      ref={dialog}
      role="dialog"
      aria-modal="true"
      aria-label={props.image.filename ?? t("drift.lightbox.image")}
      tabIndex={-1}
      data-modal-layer
      class="fixed inset-0 z-50 flex flex-col bg-black/85"
      onPointerDown={(event) => closeOnBackdropPointerDown(event, props.onClose, dialog)}
    >
      <div class="flex items-center gap-3 px-4 py-2.5 text-xs text-white/70 select-none">
        <span class="truncate text-white/90">{props.image.filename ?? t("drift.lightbox.image")}</span>
        <Show when={props.image.mime}>
          <span>{props.image.mime}</span>
        </Show>
        <Show when={natural()}>{(size) => <span>{size().w} × {size().h}</span>}</Show>
        <Show when={dataSize(props.image.url)}>{(size) => <span>{size()}</span>}</Show>
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
          onClick={props.onClose}
        >
          <IconX class="size-4" />
        </button>
      </div>
      <div
        ref={(element) => {
          viewport = element
          element.addEventListener(
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
        <div
          class="flex min-h-full min-w-fit items-center justify-center p-8"
          onPointerDown={(event) => closeOnBackdropPointerDown(event, props.onClose, dialog)}
        >
          <img
            src={props.image.url}
            alt={props.image.filename ?? t("drift.lightbox.image")}
            class="select-none"
            style={{ width: width(), "max-width": natural() ? undefined : "90vw" }}
            onLoad={(event) => measure(event.currentTarget)}
            onDblClick={() => setZoom(zoom() === 1 ? clampZoom(1 / fitScale()) : 1)}
          />
        </div>
      </div>
    </div>
  )
}
