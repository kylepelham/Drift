/**
 * Pure helpers for window-level file drag-and-drop in the composer.
 *
 * The desktop shell disables Tauri's native drop interception (`dragDropEnabled: false` in
 * tauri.conf.json) so WebView2 delivers standard HTML5 drag events with real `File` objects,
 * letting drops share the exact ingestion pipeline the paste handler and file picker use.
 */

export type DragDepthTransition = "enter" | "leave" | "drop" | "end"

/** Only drags carrying OS files count; text-selection and intra-app drags are ignored. */
export function dragHasFiles(types: readonly string[] | DOMStringList | null | undefined) {
  return !!types && Array.from(types).includes("Files")
}

/**
 * dragenter/dragleave fire for every child element the cursor crosses, so a naive boolean
 * flickers. Only the enter/leave balance matters; drop and dragend always reset it.
 */
export function nextDragDepth(depth: number, transition: DragDepthTransition) {
  if (transition === "enter") return depth + 1
  if (transition === "leave") return Math.max(0, depth - 1)
  return 0
}

export function dropTargetActive(depth: number) {
  return depth > 0
}

/**
 * Whether a drop belongs to the composer rather than to whatever is open above it.
 *
 * The listeners are window-level so a drop anywhere over the chat attaches, but dialogs render on
 * top of that chat. A file dropped into one is meant for that dialog, or for nothing at all, and
 * must not be staged into the composer hidden behind it.
 */
export function dropStagesAttachment(target: EventTarget | null) {
  // Duck-typed rather than an `instanceof Element` check: the window itself and text nodes are
  // both legitimate targets that carry no `closest`, and neither sits inside a dialog.
  const element = target as { closest?: (selectors: string) => unknown } | null
  if (typeof element?.closest !== "function") return true
  return !element.closest('[role="dialog"]')
}

export type DroppedItemLike = {
  kind: string
  getAsFile(): File | null
  webkitGetAsEntry?(): { isDirectory: boolean } | null
}

/**
 * Separates real files from directory entries: directories arrive as `File` objects with no
 * usable content, so they are counted for a notice instead of silently failing to read.
 * `fallback` covers environments whose DataTransfer populates `files` but not `items`.
 */
export function splitDroppedFiles(items: readonly DroppedItemLike[], fallback: readonly File[] = []) {
  const files: File[] = []
  let directories = 0
  for (const item of items) {
    if (item.kind !== "file") continue
    if (item.webkitGetAsEntry?.()?.isDirectory) {
      directories++
      continue
    }
    const file = item.getAsFile()
    if (file) files.push(file)
  }
  if (!files.length && !directories && fallback.length) return { files: [...fallback], directories: 0 }
  return { files, directories }
}
