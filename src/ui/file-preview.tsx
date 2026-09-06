import { createEffect, createMemo, createSignal, For, lazy, Match, onCleanup, onMount, Show, Suspense, Switch } from "solid-js"
import { Portal } from "solid-js/web"
import { filePreviewMime } from "../file-preview-types"
import { previewTable, readFilePreview } from "../file-preview"
import { closeFilePreview, previewFile, type FilePreviewRequest } from "../state/file-preview"
import { t } from "../state/i18n"
import { openFile } from "../tool-actions"
import { IconX } from "./icons"
import { ImageViewer } from "./image-viewer"
import { ProgressiveCodeView } from "./markdown"
import { MarkdownDocument } from "./markdown-document"
import { activateModal, closeOnBackdropPointerDown } from "./modal"

const PdfPreview = lazy(() => import("./pdf-preview").then((module) => ({ default: module.PdfPreview })))

export function FilePreviewHost() {
  return <Show when={previewFile()} keyed>{(file) => <Portal><FilePreviewDialog file={file} /></Portal>}</Show>
}

function FilePreviewDialog(props: { file: FilePreviewRequest }) {
  let dialog!: HTMLDivElement
  const [attempt, setAttempt] = createSignal(0)
  const [loaded, setLoaded] = createSignal<Awaited<ReturnType<typeof readFilePreview>>>()
  const [url, setUrl] = createSignal<string>()
  const [error, setError] = createSignal("")
  const [editorError, setEditorError] = createSignal("")
  const filename = () => props.file.path.split(/[\\/]/).pop() ?? props.file.path
  onMount(() => onCleanup(activateModal(dialog, closeFilePreview, { nativeTabOrder: true })))

  createEffect(() => {
    attempt()
    let disposed = false
    let objectUrl: string | undefined
    setLoaded(undefined)
    setUrl(undefined)
    setError("")
    onCleanup(() => {
      disposed = true
      if (objectUrl) URL.revokeObjectURL(objectUrl)
    })
    void readFilePreview(props.file).then((result) => {
      if (disposed) return
      if (result.kind === "image" || result.kind === "audio" || result.kind === "video") {
        objectUrl = URL.createObjectURL(new Blob([result.bytes], { type: filePreviewMime(props.file.path) }))
        setUrl(objectUrl)
      }
      setLoaded(result)
    }).catch((cause: unknown) => {
      if (!disposed) setError(cause instanceof Error ? cause.message : typeof cause === "string" ? cause : t("drift.preview.error"))
    })
  })

  async function editor() {
    setEditorError("")
    try {
      await openFile(props.file.path, { line: props.file.line, column: props.file.column, editorOnly: true })
    } catch (cause) {
      setEditorError(cause instanceof Error ? cause.message : typeof cause === "string" ? cause : t("drift.preview.error"))
    }
  }

  return (
    <div ref={dialog} role="dialog" aria-modal="true" aria-label={`${t("drift.preview.title")}: ${filename()}`}
      tabIndex={-1} data-modal-layer class="fixed inset-0 z-50 flex items-center justify-center bg-black/65 p-2 sm:p-6"
      onPointerDown={(event) => closeOnBackdropPointerDown(event, closeFilePreview, dialog)}>
      <section class="flex h-[90dvh] max-h-[1000px] w-full max-w-6xl min-w-0 flex-col overflow-hidden rounded-xl border border-edge bg-bg text-ink shadow-2xl">
        <header class="flex shrink-0 flex-wrap items-center gap-2 border-b border-edge px-3 py-2 sm:px-4">
          <div class="min-w-0 flex-1">
            <h2 class="truncate text-sm font-medium" title={props.file.path}>{filename()}</h2>
            <p class="truncate text-[0.65rem] text-ink-faint" title={props.file.path}>{props.file.path}</p>
          </div>
          <button class="rounded px-2 py-1 text-xs text-ink-muted hover:bg-raised hover:text-ink" onClick={() => void editor()}>{t("drift.preview.openEditor")}</button>
          <button class="flex size-8 shrink-0 items-center justify-center rounded hover:bg-raised" aria-label={t("common.close")} onClick={closeFilePreview}><IconX class="size-4" /></button>
        </header>
        <Show when={editorError()}><p role="alert" class="px-4 py-2 text-sm text-red-400">{editorError()}</p></Show>
        <Show when={!error()} fallback={
          <div class="m-auto max-w-lg p-6 text-center">
            <p role="alert" class="break-words text-sm text-ink-muted">{error()}</p>
            <button class="mt-4 rounded border border-edge px-3 py-1.5 text-xs hover:bg-raised" onClick={() => setAttempt((value) => value + 1)}>{t("drift.preview.retry")}</button>
          </div>
        }>
          <Show when={loaded()} fallback={<p role="status" class="m-auto p-6 text-sm text-ink-muted">{t("drift.preview.loading")}</p>}>
            {(file) => (
              <Switch>
                <Match when={file().kind === "markdown"}>
                  <div class="min-h-0 flex-1 overflow-auto px-4 py-5 sm:px-8"><div class="mx-auto max-w-4xl">
                    <MarkdownDocument text={file().text!} path={props.file.path} directory={props.file.directory} hash={props.file.hash} />
                  </div></div>
                </Match>
                <Match when={file().kind === "text"}>
                  <div class="flex min-h-0 flex-1 flex-col p-3"><ProgressiveCodeView code={file().text!} filename={filename()} line={props.file.line} fill /></div>
                </Match>
                <Match when={file().kind === "table"}><TablePreview text={file().text!} path={props.file.path} /></Match>
                <Match when={file().kind === "pdf"}>
                  <Suspense fallback={<p role="status" class="m-auto p-6 text-sm">{t("drift.preview.loading")}</p>}>
                    <PdfPreview data={file().bytes} initialPage={Number(/^page=(\d+)$/.exec(props.file.hash ?? "")?.[1]) || 1} />
                  </Suspense>
                </Match>
                <Match when={file().kind === "image"}>
                  <ImageViewer src={url()!} alt={filename()} onError={() => setError(t("drift.preview.mediaError"))} />
                </Match>
                <Match when={file().kind === "audio"}>
                  <div class="grid min-h-0 flex-1 place-items-center p-6"><audio src={url()} controls tabIndex={0} preload="metadata" class="w-full max-w-2xl" aria-label={filename()} onError={() => setError(t("drift.preview.mediaError"))} /></div>
                </Match>
                <Match when={file().kind === "video"}>
                  <div class="flex min-h-0 flex-1 items-center justify-center bg-black p-2"><video src={url()} controls tabIndex={0} playsinline preload="metadata" class="max-h-full max-w-full" aria-label={filename()} onError={() => setError(t("drift.preview.mediaError"))} /></div>
                </Match>
              </Switch>
            )}
          </Show>
        </Show>
      </section>
    </div>
  )
}

function TablePreview(props: { text: string; path: string }) {
  const table = createMemo(() => previewTable(props.text, /\.tsv$/i.test(props.path) ? "\t" : ","))
  return (
    <div class="min-h-0 flex-1 overflow-auto p-4">
      <Show when={table().truncated}><p class="mb-3 text-xs text-ink-muted">{t("drift.preview.tableTruncated", { rows: table().rows.length })}</p></Show>
      <table class="w-full border-collapse text-left text-xs"><tbody>
        <For each={table().rows}>{(row, index) => <tr classList={{ "bg-raised font-medium": index() === 0 }}>
          <For each={row}>{(cell) => <td class="max-w-md whitespace-pre-wrap break-words border border-edge px-3 py-2">{cell}</td>}</For>
        </tr>}</For>
      </tbody></table>
    </div>
  )
}
