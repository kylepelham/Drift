import { createSignal } from "solid-js"
import { shellEvents } from "../shell"

/** How far the shell's import of opencode conversations has got; `null` when none is running. */
export type ImportProgress = { done: number; total: number }

const [progress, setProgress] = createSignal<ImportProgress | null>(null)

export { progress as opencodeImport }

/** Shows each step the shell reports, and nothing once the last conversation is in. */
export function acceptImportProgress(next: ImportProgress) {
  setProgress(next.total > 0 && next.done < next.total ? next : null)
}

let listening = false

export function listenOpencodeImport() {
  if (listening) return
  listening = true
  void shellEvents()?.listen<ImportProgress>("opencode-import", (event) => acceptImportProgress(event.payload))
}
