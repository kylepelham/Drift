import DOMPurify from "dompurify"
import { marked } from "marked"
import { createEffect, createMemo, createSignal, Show } from "solid-js"
import { theme } from "../state/theme"

marked.use({ gfm: true, breaks: true })

let shikiModule: Promise<typeof import("shiki")> | undefined

async function highlightBlocks(root: HTMLElement) {
  const shiki = await (shikiModule ??= import("shiki"))
  const shikiTheme = theme() === "drift-light" ? "github-light" : "github-dark-default"
  for (const code of root.querySelectorAll<HTMLElement>("pre > code[class*='language-']")) {
    const lang = code.className.match(/language-([\w-]+)/)?.[1] ?? "text"
    const pre = code.parentElement
    if (!pre) continue
    await shiki
      .codeToHtml(code.textContent ?? "", { lang, theme: shikiTheme })
      .then((html) => (pre.outerHTML = html))
      .catch(() => {})
  }
}

// Model output like **C:\** never closes its emphasis because \ escapes the delimiter.
// Only path-shaped backslashes (after a drive colon or another segment) get self-escaped.
export function fixEscapedEmphasis(text: string) {
  return text
    .split(/(```[\s\S]*?```|`[^`\n]*`)/g)
    .map((chunk, index) => {
      if (index % 2) return chunk
      return chunk
        .replace(/(:)\\(?=\*\*?|__?)/g, "$1\\\\")
        .replace(/(\\[\w .()-]+)\\(?=\*\*?|__?)/g, "$1\\\\")
    })
    .join("")
}

function openExternalLink(event: MouseEvent) {
  const anchor = (event.target as Element).closest<HTMLAnchorElement>("a[href]")
  if (!anchor) return
  const url = new URL(anchor.href)
  if (url.protocol !== "http:" && url.protocol !== "https:") return
  const invoke = (globalThis as { __TAURI__?: { core?: { invoke: (command: string, args: unknown) => Promise<unknown> } } })
    .__TAURI__?.core?.invoke
  if (!invoke) return
  event.preventDefault()
  void invoke("plugin:opener|open_url", { url: url.href }).catch(() => {})
}

export function CodeView(props: { code: string; lang: string }) {
  const [html, setHtml] = createSignal("")
  createEffect(() => {
    const { code, lang } = props
    const shikiTheme = theme() === "drift-light" ? "github-light" : "github-dark-default"
    void (shikiModule ??= import("shiki"))
      .then((shiki) => shiki.codeToHtml(code, { lang, theme: shikiTheme }))
      .then((output) => setHtml(DOMPurify.sanitize(output)))
      .catch(() => setHtml(""))
  })
  return (
    <Show when={html()} fallback={<pre>{props.code}</pre>}>
      <div innerHTML={html()} />
    </Show>
  )
}

export function Markdown(props: { text: string; done?: boolean }) {
  let root!: HTMLDivElement
  const html = createMemo(() => DOMPurify.sanitize(marked.parse(fixEscapedEmphasis(props.text), { async: false })))
  createEffect(() => {
    if (html() && props.done) void highlightBlocks(root)
  })
  return <div ref={root} class="md" innerHTML={html()} onClick={openExternalLink} />
}
