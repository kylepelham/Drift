import { createEffect, onCleanup } from "solid-js"

export type DismissOptions = {
  /**
   * Elements that count as "inside" the surface. A pointer press on any of them is ignored.
   * Returned as a function because refs are assigned after the effect first runs, and some
   * surfaces (a trigger plus a portalled panel) have more than one root.
   */
  inside: () => (Node | null | undefined)[]
  onDismiss: () => void
  /** Dismiss when Escape is pressed anywhere in the document. */
  escape?: boolean
  /**
   * Dismiss when the page scrolls. Menus positioned at fixed viewport coordinates use this because
   * they cannot follow their anchor once it moves. Listens in the capture phase so scrolling in a
   * nested container still counts.
   */
  scroll?: boolean
  /** Dismiss when the window loses focus. */
  blur?: boolean
  /** Dismiss when the window is resized, which invalidates a fixed position. */
  resize?: boolean
  /**
   * When provided and false, no listeners are attached at all. Surfaces that stay mounted while
   * hidden use this so a closed surface costs nothing.
   */
  enabled?: () => boolean
}

/**
 * Closes a floating surface when the user interacts outside of it.
 *
 * Every option defaults to off so a caller listens to exactly the events it needs. The effect
 * re-subscribes whenever its reactive dependencies change and unsubscribes on cleanup.
 */
export function createDismissOnOutside(options: DismissOptions) {
  createEffect(() => {
    if (options.enabled && !options.enabled()) return
    const dismiss = () => options.onDismiss()

    const onPointerDown = (event: MouseEvent) => {
      const target = event.target as Node
      const insideAny = options.inside().some((node) => node?.contains(target))
      if (!insideAny) dismiss()
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") dismiss()
    }

    document.addEventListener("mousedown", onPointerDown)
    if (options.escape) document.addEventListener("keydown", onKeyDown)
    if (options.scroll) document.addEventListener("scroll", dismiss, true)
    if (options.blur) window.addEventListener("blur", dismiss)
    if (options.resize) window.addEventListener("resize", dismiss)

    onCleanup(() => {
      document.removeEventListener("mousedown", onPointerDown)
      if (options.escape) document.removeEventListener("keydown", onKeyDown)
      if (options.scroll) document.removeEventListener("scroll", dismiss, true)
      if (options.blur) window.removeEventListener("blur", dismiss)
      if (options.resize) window.removeEventListener("resize", dismiss)
    })
  })
}
