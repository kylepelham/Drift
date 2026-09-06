import { createSignal, onCleanup, onMount, Show } from "solid-js"
import { Portal } from "solid-js/web"
import { t } from "../state/i18n"
import { IconX } from "./icons"
import { ImageViewer } from "./image-viewer"
import { activateModal, closeOnBackdropPointerDown } from "./modal"

type LightboxImage = { url: string; filename?: string; mime?: string; blob?: Blob }

const [image, setImage] = createSignal<LightboxImage | null>(null)
let ownedUrl: string | undefined

export function openLightbox(next: LightboxImage) {
  const url = next.blob ? URL.createObjectURL(next.blob) : undefined
  if (ownedUrl) URL.revokeObjectURL(ownedUrl)
  ownedUrl = url
  setImage({ ...next, url: url ?? next.url })
}

function closeLightbox() {
  if (ownedUrl) URL.revokeObjectURL(ownedUrl)
  ownedUrl = undefined
  setImage(null)
}

function dataSize(url: string) {
  if (!url.startsWith("data:")) return null
  const bytes = Math.round(((url.length - url.indexOf(",") - 1) * 3) / 4)
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
  return `${Math.max(1, Math.round(bytes / 1024))} KB`
}

export function Lightbox() {
  onCleanup(closeLightbox)
  return (
    <Show when={image()} keyed>
      {(item) => (
        <Portal>
          <LightboxDialog image={item} onClose={closeLightbox} />
        </Portal>
      )}
    </Show>
  )
}

function LightboxDialog(props: { image: LightboxImage; onClose: () => void }) {
  let dialog!: HTMLDivElement
  onMount(() => onCleanup(activateModal(dialog, props.onClose)))

  return (
    <div ref={dialog} role="dialog" aria-modal="true" aria-label={props.image.filename ?? t("drift.lightbox.image")}
      tabIndex={-1} data-modal-layer class="fixed inset-0 z-50 flex flex-col bg-bg/95"
      onPointerDown={(event) => closeOnBackdropPointerDown(event, props.onClose, dialog)}>
      <ImageViewer src={props.image.url} alt={props.image.filename ?? t("drift.lightbox.image")} onBackgroundClick={props.onClose}
        toolbarStart={<span class="min-w-0 flex-1 truncate text-ink" title={props.image.filename}>{props.image.filename ?? t("drift.lightbox.image")}</span>}
        toolbarEnd={
          <div class="flex shrink-0 items-center gap-2 text-ink-muted">
            <Show when={props.image.mime}><span class="hidden max-w-32 truncate lg:inline" title={props.image.mime}>{props.image.mime}</span></Show>
            <Show when={dataSize(props.image.url)}>{(size) => <span class="hidden shrink-0 lg:inline">{size()}</span>}</Show>
            <button type="button" aria-label={t("common.close")} title={t("common.close")} class="flex size-8 shrink-0 items-center justify-center rounded hover:bg-raised hover:text-ink" onClick={props.onClose}>
              <IconX class="size-4" />
            </button>
          </div>
        } />
    </div>
  )
}
