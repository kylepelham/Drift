import { shellEvents, shellInvoke } from "../shell"
import { createSignal } from "solid-js"

/** How far the shell's import of opencode conversations has got; `null` when none is running. */
export type ImportProgress = { done: number; total: number }

/** What an import brought in and left behind, shown once; names only, the window words the rest. */
export type ImportSummary = {
  conversations: number
  undoable: number
  pending: string[]
  waiting: Record<string, number>
  signIns: string[]
  servers: string[]
  serversOff: string[]
  files: number
  leftOut: { signIns: string[]; plugins: string[]; settings: string[]; servers: string[]; failed: string[] }
  failed: number
}

const [progress, setProgress] = createSignal<ImportProgress | null>(null)
const [summary, setSummary] = createSignal<ImportSummary | null>(null)

export { progress as opencodeImport, summary as importSummary }

/** Shows each step the shell reports, and nothing once the last conversation is in. */
export function acceptImportProgress(next: ImportProgress) {
  setProgress(next.total > 0 && next.done < next.total ? next : null)
}

export function dismissImportSummary() {
  setSummary(null)
}

/** The shell hands a summary out once, so whichever of startup or its event asks first shows it. */
async function takeSummary() {
  const taken = await shellInvoke()?.<ImportSummary | null>("opencode_import_summary").catch(() => null)
  if (taken) setSummary(taken)
}

let listening = false

export function listenOpencodeImport() {
  if (listening) return
  listening = true
  const events = shellEvents()
  void events?.listen<ImportProgress>("opencode-import", (event) => acceptImportProgress(event.payload))
  void events?.listen("opencode-import-done", () => void takeSummary())
  void takeSummary()
}
