import { readFilePreview } from "../file-preview"
import { filePreviewLimits, filePreviewMime, filePreviewType } from "../file-preview-types"
import { classifyMarkdownLink } from "./markdown-links"
import { openLightbox } from "./lightbox"

export const markdownImageAttribute = "data-document-image"

export function observeMarkdownImages(root: HTMLDivElement, input: { parent?: string; directory?: string; enabled: boolean; hash?: string }): () => void {
  const { parent, directory, enabled } = input
  const hash = input.hash?.replace(/^#/, "")
  let disposed = false
  let running = false
  let pending = false
  let scrolled = false
  let remaining = 20 * 1024 ** 2
  const seen = new WeakSet<HTMLImageElement>()
  const cache = new Map<string, { url?: string; blob?: Blob; error?: string }>()
  const urls = new Map<HTMLImageElement, string>()
  const controls = new Map<HTMLImageElement, (string | null)[]>()
  const controlAttributes = ["role", "tabindex", "class"]
  let aligning = !!hash
  let alignmentFrame: number | undefined
  let framesLeft = 0
  let alignmentBudget = 24
  const ownerDocument = root.ownerDocument
  const interactionEvents = ["wheel", "touchstart", "pointerdown", "keydown", "click"] as const

  function imageSource(image: HTMLImageElement) {
    const url = image.getAttribute("src") ?? ""
    const cached = [...cache.values()].find((item) => item.url === url && !item.error)
    if (cached?.blob) return { url, blob: cached.blob }
    if (!image.getAttribute(markdownImageAttribute) &&
      (classifyMarkdownLink(url).kind === "external" || /^data:image\//i.test(url))) return { url }
  }

  function restoreControl(image: HTMLImageElement) {
    controls.get(image)?.forEach((value, index) => {
      if (value === null) image.removeAttribute(controlAttributes[index])
      else image.setAttribute(controlAttributes[index], value)
    })
    controls.delete(image)
  }

  function decorate(image: HTMLImageElement) {
    if (controls.has(image) || image.closest("a[href],button") || !imageSource(image)) return
    controls.set(image, controlAttributes.map((name) => image.getAttribute(name)))
    image.setAttribute("role", "button")
    image.setAttribute("tabindex", "0")
    image.setAttribute("class", `${image.getAttribute("class") ?? ""} cursor-zoom-in focus-visible:outline-2 focus-visible:outline-accent`)
  }

  function activateImage(event: MouseEvent | KeyboardEvent) {
    if (event.defaultPrevented || event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) return
    if (event.type === "click" ? (event as MouseEvent).button !== 0 :
      (event as KeyboardEvent).repeat || !["Enter", " "].includes((event as KeyboardEvent).key)) return
    const image = (event.target as Element | null)?.closest<HTMLImageElement>("img")
    if (!image || !root.contains(image) || !controls.has(image) || image.closest("a[href],button") || !image.complete || !image.naturalWidth) return
    const source = imageSource(image)
    if (!source) return
    event.preventDefault()
    event.stopPropagation()
    image.focus({ preventScroll: true })
    // The lightbox owns a separate URL so transcript cleanup cannot invalidate its image.
    openLightbox({ ...source, filename: image.getAttribute("alt") || undefined, mime: source.blob?.type })
  }

  root.addEventListener("click", activateImage)
  root.addEventListener("keydown", activateImage)

  function stopAlignment() {
    aligning = false
    if (alignmentFrame !== undefined) cancelAnimationFrame(alignmentFrame)
    alignmentFrame = undefined
  }

  function alignHash() {
    if (disposed || !aligning) return
    const target = [...root.querySelectorAll<HTMLElement>("[id]")].find((element) => element.id === hash)
    if (target) { scrolled = true; target.scrollIntoView({ block: "start", behavior: "instant" }) }
  }

  function settleAlignment() {
    if (disposed || !aligning || alignmentBudget <= 0) return
    framesLeft = 2
    if (alignmentFrame !== undefined) return
    const tick = () => {
      alignmentFrame = undefined
      if (disposed || !aligning || alignmentBudget <= 0) return
      alignHash()
      alignmentBudget--
      if (--framesLeft > 0 && alignmentBudget > 0) alignmentFrame = requestAnimationFrame(tick)
    }
    alignmentFrame = requestAnimationFrame(tick)
  }

  // Capture input on the owning document, including the enclosing scroller and its scrollbar.
  // Do not listen for scroll: our own alignment and browser scroll anchoring also emit it.
  if (hash) for (const type of interactionEvents)
    ownerDocument.addEventListener(type, stopAlignment, { capture: true, passive: true })

  async function update() {
    if (disposed) return
    if (!scrolled) alignHash()
    for (const [image] of controls) if (!root.contains(image)) restoreControl(image)
    for (const image of root.querySelectorAll<HTMLImageElement>("img")) decorate(image)
    for (const [image] of urls) {
      if (root.contains(image)) continue
      image.onload = null
      image.onerror = null
      image.removeAttribute("src")
      urls.delete(image)
    }
    if (running) { pending = true; return }
    running = true
    try {
      for (const image of root.querySelectorAll<HTMLImageElement>(`img[${markdownImageAttribute}]`)) {
        if (disposed) return
        if (seen.has(image) || !root.contains(image)) continue
        seen.add(image)
        const raw = image.getAttribute(markdownImageAttribute) ?? ""
        const link = classifyMarkdownLink(raw, parent)
        if (!enabled || !parent || !directory || link.kind !== "file" || link.path.startsWith("//") || filePreviewType(link.path) !== "image") {
          image.title = "Only enabled local workspace images can be previewed"
          continue
        }
        let cached = cache.get(link.path)
        if (!cached) {
          // Reserve the reader's worst-case allocation once per path, including failed reads.
          if (cache.size >= 12 || remaining < filePreviewLimits.image) {
            image.title = "Document image preview limit reached"
            continue
          }
          cached = {}
          cache.set(link.path, cached)
          remaining -= filePreviewLimits.image
          try {
            const result = await readFilePreview({ path: link.path, directory })
            if (disposed) return
            if (result.kind !== "image" || result.bytes.byteLength > filePreviewLimits.image)
              throw new Error("Unsupported image preview")
            remaining += filePreviewLimits.image - result.bytes.byteLength
            // Keep bounded URLs across DOM replacement. SVG is only used in image context.
            cached.blob = new Blob([result.bytes], { type: filePreviewMime(link.path) })
            cached.url = URL.createObjectURL(cached.blob)
          } catch {
            cached.error = "Image could not be read within the workspace preview limits"
          }
        }
        if (disposed) return
        if (!root.contains(image)) continue
        if (cached.error) {
          image.title = cached.error
          continue
        }
        const url = cached.url!
        urls.set(image, url)
        // The read only supplies bytes. Wait for the image's actual load before re-aligning.
        image.onload = () => {
          if (disposed || urls.get(image) !== url || !root.contains(image)) return
          image.onload = null
          settleAlignment()
        }
        image.onerror = () => {
          if (disposed || urls.get(image) !== url || !root.contains(image)) return
          image.onload = null
          image.onerror = null
          image.removeAttribute("src")
          image.title = cached.error = "Image could not be previewed"
          urls.delete(image)
          restoreControl(image)
          settleAlignment()
        }
        image.src = url
        decorate(image)
      }
    } finally {
      running = false
      if (pending) {
        pending = false
        queueMicrotask(() => { void update() })
      }
    }
  }

  // Only child replacement matters. Our src/title changes must not trigger new reads.
  const observer = new MutationObserver(() => { void update() })
  observer.observe(root, { childList: true, subtree: true })
  queueMicrotask(() => { void update() })
  return () => {
    disposed = true
    stopAlignment()
    if (hash) for (const type of interactionEvents) ownerDocument.removeEventListener(type, stopAlignment, true)
    observer.disconnect()
    root.removeEventListener("click", activateImage)
    root.removeEventListener("keydown", activateImage)
    for (const [image] of controls) restoreControl(image)
    for (const [image] of urls) {
      image.onload = null
      image.onerror = null
      image.removeAttribute("src")
    }
    urls.clear()
    for (const { url } of cache.values()) if (url) URL.revokeObjectURL(url)
    cache.clear()
  }
}
