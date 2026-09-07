import { backendInvoke } from "./backend"
import { filePreviewLimits, filePreviewType } from "./file-preview-types"
import { t } from "./state/i18n"
import type { FilePreviewRequest } from "./state/file-preview"

export async function readFilePreview(request: FilePreviewRequest) {
  const kind = filePreviewType(request.path)
  if (!kind) throw new Error(t("drift.preview.unsupported"))
  const invoke = backendInvoke()
  if (!invoke) throw new Error(t("drift.preview.unavailable"))
  const limit = filePreviewLimits[kind]
  const result = await invoke<{ content: string; size: number }>("read_file_preview", {
    path: request.path,
    directory: request.directory,
    maxBytes: limit,
  })
  if (!result || !Number.isSafeInteger(result.size) || result.size < 0 || typeof result.content !== "string")
    throw new Error(t("drift.preview.error"))
  if (result.size > limit || result.content.length > Math.ceil(limit / 3) * 4)
    throw new Error(t("drift.preview.tooLarge"))
  const binary = atob(result.content)
  if (binary.length !== result.size) throw new Error(t("drift.preview.error"))
  const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0))
  const text = kind === "markdown" || kind === "text" || kind === "table"
    ? new TextDecoder("utf-8", { fatal: true }).decode(bytes)
    : undefined
  if (text?.includes("\0")) throw new Error(t("drift.preview.unsupported"))
  return { kind, bytes, text }
}

export function previewParentDirectory(path: string) {
  const normalized = path.replaceAll("\\", "/")
  const split = normalized.lastIndexOf("/")
  return split <= 0 ? "/" : /^[a-z]:\/$/i.test(normalized.slice(0, split + 1))
    ? normalized.slice(0, split + 1)
    : normalized.slice(0, split)
}

// Limit rendered cells and stop parsing once the visible row window is full.
export function previewTable(text: string, delimiter: string, rowLimit = 200, columnLimit = 50) {
  const rows: string[][] = []
  let row: string[] = []
  let field = ""
  let quoted = false
  let truncated = false
  const cell = () => {
    if (row.length < columnLimit) row.push(field)
    else truncated = true
    field = ""
  }
  for (let index = 0; index < text.length; index++) {
    const character = text[index]
    if (character === '"') {
      if (quoted && text[index + 1] === '"') { field += '"'; index++ }
      else quoted = !quoted
    } else if (!quoted && character === delimiter) cell()
    else if (!quoted && (character === "\n" || character === "\r")) {
      if (character === "\r" && text[index + 1] === "\n") index++
      cell()
      rows.push(row)
      row = []
      if (rows.length >= rowLimit) return { rows, truncated: truncated || index + 1 < text.length }
    } else field += character
  }
  if (field || row.length) { cell(); rows.push(row) }
  return { rows, truncated }
}
