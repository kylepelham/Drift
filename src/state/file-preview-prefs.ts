import { filePreviewType, filePreviewTypes, type FilePreviewType } from "../file-preview-types"
import { persisted } from "./persist"

export type FilePreviewPrefs = {
  mode: "all" | "none" | "custom"
  types: Record<FilePreviewType, boolean>
}

export function normalizeFilePreviewPrefs(value: unknown): FilePreviewPrefs {
  const stored = value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {}
  const types = stored.types && typeof stored.types === "object" && !Array.isArray(stored.types)
    ? stored.types as Record<string, unknown>
    : {}
  return {
    mode: stored.mode === "none" || stored.mode === "custom" ? stored.mode : "all",
    types: Object.fromEntries(filePreviewTypes.map((type) => [
      type,
      Object.hasOwn(types, type) && typeof types[type] === "boolean" ? types[type] : true,
    ])) as Record<FilePreviewType, boolean>,
  }
}

const [filePreviewPrefs, setPrefs] = persisted<FilePreviewPrefs>(
  "drift.preview.prefs",
  normalizeFilePreviewPrefs(undefined),
  normalizeFilePreviewPrefs,
)
export { filePreviewPrefs }

export function setFilePreviewMode(mode: FilePreviewPrefs["mode"]) {
  if (mode !== "all" && mode !== "none" && mode !== "custom") return
  setPrefs({ ...filePreviewPrefs(), mode })
}

export function setFilePreviewType(type: FilePreviewType, enabled: boolean) {
  if (!filePreviewTypes.includes(type) || typeof enabled !== "boolean") return
  setPrefs({ ...filePreviewPrefs(), types: { ...filePreviewPrefs().types, [type]: enabled } })
}

export function filePreviewAllowed(path: string, prefs: FilePreviewPrefs): boolean {
  const type = filePreviewType(path)
  return type !== undefined && (prefs.mode === "all" || (prefs.mode === "custom" && prefs.types[type]))
}

export function shouldPreviewFile(path: string): boolean {
  return filePreviewAllowed(path, filePreviewPrefs())
}
