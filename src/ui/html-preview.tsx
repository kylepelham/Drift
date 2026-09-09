import createDOMPurify from "dompurify"
import { createEffect, createMemo, createSignal, createUniqueId, For, onCleanup, Show } from "solid-js"
import { previewParentDirectory } from "../file-preview"
import { shouldPreviewFile } from "../state/file-preview-prefs"
import { t } from "../state/i18n"
import { ProgressiveCodeView } from "./markdown"
import { markdownImageAttribute, observeMarkdownImages } from "./markdown-images"
import { classifyMarkdownLink } from "./markdown-links"

export function htmlPreviewDocument(text: string) {
  // Keep this policy separate from the transcript sanitizer's shared hooks.
  const purify = createDOMPurify(window)
  const images = new WeakMap<Node, string>()
  purify.addHook("uponSanitizeAttribute", (node, attribute) => {
    if (attribute.attrName === "src" && (node.nodeName !== "IMG" || !/^data:image\/(?:png|jpeg|gif|webp|avif|bmp|x-icon|svg\+xml)[;,]/i.test(attribute.attrValue))) {
      attribute.keepAttr = false
      const link = classifyMarkdownLink(attribute.attrValue, "/")
      if (node.nodeName === "IMG" && node.namespaceURI === "http://www.w3.org/1999/xhtml" && link.kind === "file" && !link.path.startsWith("//")) {
        images.set(node, attribute.attrValue)
      }
    }
  })
  const html = purify.sanitize(text, {
    WHOLE_DOCUMENT: true,
    RETURN_DOM: true,
    FORBID_TAGS: ["script", "iframe", "object", "embed", "base", "link", "meta", "form", "input", "button", "textarea", "select", "audio", "video", "source", "track", "animate", "animatemotion", "animatetransform", "set"],
    FORBID_ATTR: ["href", "xlink:href", "srcset", "srcdoc", "action", "formaction", "target", "download", "ping", "autofocus", "tabindex", "contenteditable", markdownImageAttribute],
  }) as HTMLElement
  // Only src values captured by this sanitizer can request workspace reads.
  for (const image of html.querySelectorAll("img")) {
    const raw = images.get(image)
    if (raw) image.setAttribute(markdownImageAttribute, raw)
  }
  const head = html.querySelector("head")!
  // These attributes would be parsed before the CSP. Stylesheets in head still work.
  html.removeAttribute("style")
  head.removeAttribute("style")
  const policy = html.ownerDocument.createElement("meta")
  policy.httpEquiv = "Content-Security-Policy"
  policy.content = "default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; base-uri 'none'; form-action 'none'"
  // The CSP must precede user styles, including CSS imports and url() resources.
  head.prepend(policy)
  const defaults = html.ownerDocument.createElement("style")
  defaults.textContent = "html { color-scheme: light; } body { margin: 24px; font: 16px/1.6 system-ui, sans-serif; overflow-wrap: anywhere; } img, svg { max-width: 100%; } pre { overflow: auto; }"
  policy.after(defaults)
  return `<!doctype html>\n${html.outerHTML}`
}

export function HtmlPreview(props: { text: string; filename: string; path: string; directory: string; line?: number }) {
  const id = createUniqueId()
  const [source, setSource] = createSignal(false)
  const document = createMemo(() => htmlPreviewDocument(props.text))

  return (
    <div class="flex min-h-0 min-w-0 flex-1 flex-col">
      <div class="flex shrink-0 items-center gap-1 border-b border-edge px-3 pt-1" role="tablist" aria-label={t("drift.preview.title")}>
        <For each={[false, true]}>{(value) => (
          <button id={`${id}-${value}`} role="tab" aria-selected={source() === value} aria-controls={`${id}-panel`}
            tabIndex={source() === value ? 0 : -1} class="border-b-2 px-3 py-2 text-xs hover:text-ink"
            classList={{ "border-accent text-ink": source() === value, "border-transparent text-ink-muted": source() !== value }}
            onClick={() => setSource(value)} onKeyDown={(event) => {
              if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return
              event.preventDefault()
              const next = event.key === "Home" ? false : event.key === "End" ? true : !value
              setSource(next)
              event.currentTarget.parentElement?.querySelector<HTMLButtonElement>(`[id="${id}-${next}"]`)?.focus()
            }}>
            {t(value ? "drift.preview.htmlSource" : "drift.preview.htmlRendered")}
          </button>
        )}</For>
      </div>
      <div id={`${id}-panel`} role="tabpanel" aria-labelledby={`${id}-${source()}`} class="flex min-h-0 flex-1 flex-col">
        <Show when={!source() && document()} keyed fallback={
          <div class="flex min-h-0 flex-1 flex-col p-3"><ProgressiveCodeView code={props.text} filename={props.filename} line={props.line} fill /></div>
        }>
          {(html) => {
            const [content, setContent] = createSignal<Document>()
            let detach: (() => void) | undefined
            onCleanup(() => detach?.())
            createEffect(() => {
              const body = content()?.body
              if (!body) return
              onCleanup(observeMarkdownImages(body, {
                parent: previewParentDirectory(props.path),
                directory: props.directory,
                enabled: shouldPreviewFile("image.png"),
                interactive: false,
              }))
            })
            return <>
              <p class="shrink-0 px-3 py-2 text-xs text-ink-faint">{t("drift.preview.htmlStatic")}</p>
              <iframe title={props.filename} sandbox="allow-same-origin" referrerPolicy="no-referrer" srcdoc={html}
                tabIndex={0} class="min-h-0 w-full flex-1 border-0 bg-white" onLoad={(event) => {
                  detach?.()
                  const frame = event.currentTarget
                  const content = frame.contentDocument
                  setContent(content ?? undefined)
                  if (!content) return
                  // Scripts remain forbidden. Same-origin lets the host forward Escape
                  // from the document to the existing topmost-modal keyboard handler.
                  const escape = (key: KeyboardEvent) => {
                    if (key.key !== "Escape") return
                    key.preventDefault()
                    frame.ownerDocument.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }))
                  }
                  content.addEventListener("keydown", escape)
                  detach = () => content.removeEventListener("keydown", escape)
                }} />
            </>
          }}
        </Show>
      </div>
    </div>
  )
}
