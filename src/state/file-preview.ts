import { createSignal } from "solid-js"

export type FilePreviewRequest = {
  path: string
  directory: string
  line?: number
  column?: number
  hash?: string
}

export const [previewFile, setPreviewFile] = createSignal<FilePreviewRequest>()

export function openFilePreview(request: FilePreviewRequest) {
  setPreviewFile({ ...request })
}

export function closeFilePreview() {
  setPreviewFile(undefined)
}
