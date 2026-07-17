import DOMPurify from "dompurify"
import { marked } from "marked"
import { createEffect, createMemo } from "solid-js"
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

export function Markdown(props: { text: string; done?: boolean }) {
  let root!: HTMLDivElement
  const html = createMemo(() => DOMPurify.sanitize(marked.parse(props.text, { async: false })))
  createEffect(() => {
    if (html() && props.done) void highlightBlocks(root)
  })
  return <div ref={root} class="md text-[0.925rem]" innerHTML={html()} />
}
