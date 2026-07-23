import DOMPurify from "dompurify"
import { marked } from "marked"
import type { BundledLanguage, SpecialLanguage } from "shiki"
import { createEffect, createMemo, createSignal, For, onCleanup, Show } from "solid-js"
import { theme } from "../state/theme"

marked.use({ gfm: true, breaks: true })

let shikiModule: Promise<typeof import("shiki")> | undefined
const codeBlocks = new WeakMap<HTMLElement, string>()
const copyIcon =
  '<svg viewBox="0 0 24 24" aria-hidden="true"><rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"/></svg>'
const copiedIcon = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M20 6 9 17l-5-5"/></svg>'

export type SyntaxToken = { content: string; color?: string; fontStyle?: number }

export async function codeTokens(code: string, lang: string): Promise<SyntaxToken[][]> {
  const shikiTheme = theme() === "drift-light" ? "github-light" : "github-dark-default"
  const shiki = await (shikiModule ??= import("shiki"))
  return shiki
    .codeToTokens(code, { lang: lang as BundledLanguage | SpecialLanguage, theme: shikiTheme })
    .then((result) =>
      result.tokens.map((line) =>
        line.map((token) => ({ content: token.content, color: token.color, fontStyle: token.fontStyle })),
      ),
    )
    .catch(() => [])
}

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

function decorateCodeBlocks(root: HTMLElement) {
  for (const pre of root.querySelectorAll<HTMLElement>("pre")) {
    const code = pre.querySelector("code")?.textContent ?? pre.textContent ?? ""
    const existing = pre.closest<HTMLElement>("[data-code-block]")
    if (existing) {
      codeBlocks.set(existing, code)
      continue
    }
    const wrapper = document.createElement("div")
    wrapper.className = "code-block"
    wrapper.dataset.codeBlock = ""
    const button = document.createElement("button")
    button.type = "button"
    button.className = "code-copy"
    button.dataset.copyCode = ""
    button.innerHTML = copyIcon
    button.setAttribute("aria-label", "Copy code")
    button.title = "Copy code"
    pre.before(wrapper)
    wrapper.append(pre, button)
    codeBlocks.set(wrapper, code)
  }
}

function markdownClick(event: MouseEvent) {
  const button = (event.target as Element).closest<HTMLButtonElement>("[data-copy-code]")
  if (!button) return openExternalLink(event)
  const wrapper = button.closest<HTMLElement>("[data-code-block]")
  const code = wrapper ? codeBlocks.get(wrapper) : undefined
  if (code === undefined) return
  void writeClipboard(code)
    .then(() => {
      button.innerHTML = copiedIcon
      button.setAttribute("aria-label", "Code copied")
      button.title = "Copied"
      setTimeout(() => {
        button.innerHTML = copyIcon
        button.setAttribute("aria-label", "Copy code")
        button.title = "Copy code"
      }, 1600)
    })
    .catch((error) => console.warn("[Drift] Could not copy code", error))
}

async function writeClipboard(text: string) {
  try {
    await navigator.clipboard.writeText(text)
    return
  } catch {
    const input = document.createElement("textarea")
    input.value = text
    input.style.position = "fixed"
    input.style.opacity = "0"
    document.body.append(input)
    input.select()
    const copied = document.execCommand("copy")
    input.remove()
    if (!copied) throw new Error("clipboard write was rejected")
  }
}

export function CodeView(props: { code: string; lang: string }) {
  const [html, setHtml] = createSignal("")
  let request = 0
  createEffect(() => {
    const { code, lang } = props
    const shikiTheme = theme() === "drift-light" ? "github-light" : "github-dark-default"
    const current = ++request
    setHtml("")
    void (shikiModule ??= import("shiki"))
      .then((shiki) => shiki.codeToHtml(code, { lang, theme: shikiTheme }))
      .then((output) => current === request && setHtml(DOMPurify.sanitize(output)))
      .catch(() => current === request && setHtml(""))
  })
  return (
    <Show when={html()} fallback={<pre>{props.code}</pre>}>
      <div innerHTML={html()} />
    </Show>
  )
}

const chunkLines = 160

export function codeChunks(code: string) {
  const lines = code.replace(/\r\n?/g, "\n").split("\n")
  const result: string[] = []
  for (let index = 0; index < lines.length; index += chunkLines)
    result.push(lines.slice(index, index + chunkLines).join("\n"))
  return result
}

export function ProgressiveCodeView(props: { code: string; lang: string }) {
  let root!: HTMLDivElement
  let observer: IntersectionObserver | undefined
  const chunks = createMemo(() => codeChunks(props.code))
  const [active, setActive] = createSignal(new Set([0]))

  createEffect(() => {
    const count = chunks().length
    queueMicrotask(() => {
      observer?.disconnect()
      if (!("IntersectionObserver" in window)) return setActive(new Set(Array.from({ length: count }, (_, index) => index)))
      observer = new IntersectionObserver(
        (entries) => {
          const visible = entries.filter((entry) => entry.isIntersecting).map((entry) => Number((entry.target as HTMLElement).dataset.chunk))
          if (!visible.length) return
          setActive((current) => new Set([...current, ...visible]))
        },
        { root, rootMargin: "320px 0px" },
      )
      for (const element of root.querySelectorAll("[data-chunk]")) observer.observe(element)
    })
  })
  onCleanup(() => observer?.disconnect())

  return (
    <div ref={root} class="code-view code-stream max-h-80 overflow-auto rounded-lg border border-edge">
      <For each={chunks()}>
        {(code, index) => (
          <div class="code-stream-chunk" data-chunk={index()}>
            <Show when={active().has(index())} fallback={<pre>{code}</pre>}>
              <CodeView code={code} lang={props.lang} />
            </Show>
          </div>
        )}
      </For>
    </div>
  )
}

export function Markdown(props: { text: string; done?: boolean }) {
  let root!: HTMLDivElement
  const html = createMemo(() => DOMPurify.sanitize(marked.parse(fixEscapedEmphasis(props.text), { async: false })))
  createEffect(() => {
    if (!html()) return
    queueMicrotask(() => decorateCodeBlocks(root))
    if (props.done) void highlightBlocks(root).then(() => decorateCodeBlocks(root))
  })
  return <div ref={root} class="md" innerHTML={html()} onClick={markdownClick} />
}
