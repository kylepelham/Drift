import DOMPurify from "dompurify"
import { marked } from "marked"
import type { BundledLanguage, BundledTheme, SpecialLanguage } from "shiki"
import { createEffect, createMemo, createSignal, For, onCleanup, Show } from "solid-js"
import { t } from "../state/i18n"
import { syntaxTheme } from "../state/code"

marked.use({ gfm: true, breaks: true })

let shikiModule: Promise<typeof import("shiki")> | undefined
const codeBlocks = new WeakMap<HTMLElement, string>()
const copyIcon =
  '<svg viewBox="0 0 24 24" aria-hidden="true"><rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"/></svg>'
const copiedIcon = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M20 6 9 17l-5-5"/></svg>'

export type SyntaxToken = { content: string; color?: string; fontStyle?: number }

type AsyncCacheEntry<T> = { value: Promise<T>; size: number }

export class AsyncSizeCache<T> {
  private entries = new Map<string, AsyncCacheEntry<T>>()
  private total = 0

  constructor(
    private limit: number,
    private outputSize: (value: T) => number,
  ) {}

  get size() {
    return this.total
  }

  get count() {
    return this.entries.size
  }

  get(key: string) {
    const entry = this.entries.get(key)
    if (!entry) return undefined
    this.entries.delete(key)
    this.entries.set(key, entry)
    return entry.value
  }

  set(key: string, sourceSize: number, value: Promise<T>) {
    const existing = this.entries.get(key)
    if (existing) this.remove(key, existing)
    if (sourceSize > this.limit) return value

    const entry: AsyncCacheEntry<T> = { value, size: sourceSize }
    const tracked = value.then(
      (result) => {
        if (this.entries.get(key) !== entry) return result
        this.total -= entry.size
        entry.size = sourceSize + this.outputSize(result)
        this.total += entry.size
        this.trim()
        return result
      },
      (error) => {
        if (this.entries.get(key) === entry) this.remove(key, entry)
        throw error
      },
    )
    entry.value = tracked
    this.entries.set(key, entry)
    this.total += entry.size
    this.trim()
    return tracked
  }

  private trim() {
    while (this.total > this.limit && this.entries.size) {
      const key = this.entries.keys().next().value
      if (key === undefined) break
      this.remove(key, this.entries.get(key)!)
    }
  }

  private remove(key: string, entry: AsyncCacheEntry<T>) {
    if (!this.entries.delete(key)) return
    this.total -= entry.size
  }
}

function tokenOutputSize(lines: SyntaxToken[][]) {
  let size = lines.length * 16
  for (const line of lines) for (const token of line) size += token.content.length + 32
  return size
}

function shikiSourceSize(key: string, code: string) {
  return key.length + code.length + 128
}

const shikiCacheBudget = 2 * 1024 * 1024
const highlightCache = new AsyncSizeCache<string>(shikiCacheBudget, (html) => html.length + 64)
const tokenCache = new AsyncSizeCache<SyntaxToken[][]>(shikiCacheBudget, tokenOutputSize)

export async function codeTokens(code: string, lang: string): Promise<SyntaxToken[][]> {
  const theme = syntaxTheme() as BundledTheme
  const key = `${theme}\0${lang}\0${code}`
  const cached = tokenCache.get(key)
  if (cached) return cached.catch(() => [])
  const result = (shikiModule ??= import("shiki"))
    .then((shiki) =>
      shiki.codeToTokens(code, {
        lang: lang as BundledLanguage | SpecialLanguage,
        theme,
      }),
    )
    .then((value) =>
      value.tokens.map((line) =>
        line.map((token) => ({ content: token.content, color: token.color, fontStyle: token.fontStyle })),
      ),
    )
  return tokenCache.set(key, shikiSourceSize(key, code), result).catch(() => [])
}

function highlightedCode(code: string, lang: string, theme: BundledTheme) {
  const key = `${theme}\0${lang}\0${code}`
  const cached = highlightCache.get(key)
  if (cached) return cached
  const result = (shikiModule ??= import("shiki"))
    .then((shiki) => shiki.codeToHtml(code, { lang, theme }))
    .then((html) => DOMPurify.sanitize(html))
  return highlightCache.set(key, shikiSourceSize(key, code), result)
}

async function highlightBlocks(root: HTMLElement, theme: BundledTheme, current: () => boolean) {
  await Promise.all(
    [...root.querySelectorAll<HTMLElement>("pre > code[class*='language-']")].map(async (code) => {
      const lang = code.className.match(/language-([\w-]+)/)?.[1] ?? "text"
      const pre = code.parentElement
      if (!pre) return
      const html = await highlightedCode(code.textContent ?? "", lang, theme).catch(() => "")
      if (html && current() && pre.isConnected) pre.outerHTML = html
    }),
  )
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

export function preserveLiteralBackslashes(text: string) {
  return text
    .split(/(```[\s\S]*?```|`[^`\n]*`)/g)
    .map((chunk, index) => (index % 2 ? chunk : chunk.replaceAll("\\", "&#92;")))
    .join("")
}

const voidHtml = new Set([
  "area",
  "base",
  "br",
  "col",
  "embed",
  "hr",
  "img",
  "input",
  "link",
  "meta",
  "param",
  "source",
  "track",
  "wbr",
])

// Marked accepts raw HTML. Escape only unmatched tags so model-written examples cannot
// turn the rest of a streamed response into one giant heading or emphasis element.
export function escapeUnbalancedHtml(text: string) {
  return text
    .split(/(```[\s\S]*?```|`[^`\n]*`)/g)
    .map((chunk, index) => (index % 2 ? chunk : escapeUnbalancedHtmlChunk(chunk)))
    .join("")
}

function escapeUnbalancedHtmlChunk(text: string) {
  const tags = [...text.matchAll(/<!--[^]*?-->|<\/?[A-Za-z][^>\n]*>/g)].map((match) => ({
    start: match.index,
    end: match.index + match[0].length,
    value: match[0],
    name: match[0].match(/^<\/?\s*([A-Za-z][\w:-]*)/)?.[1]?.toLowerCase(),
    closing: /^<\//.test(match[0]),
    matched: false,
  }))
  const stack: number[] = []
  for (let index = 0; index < tags.length; index++) {
    const tag = tags[index]
    if (!tag.name || tag.value.startsWith("<!--") || voidHtml.has(tag.name) || /\/\s*>$/.test(tag.value)) {
      tag.matched = true
      continue
    }
    if (!tag.closing) {
      stack.push(index)
      continue
    }
    let opener = -1
    for (let position = stack.length - 1; position >= 0; position--) {
      if (tags[stack[position]].name !== tag.name) continue
      opener = position
      break
    }
    if (opener < 0) continue
    const openIndex = stack[opener]
    tags[openIndex].matched = true
    tag.matched = true
    stack.splice(opener, 1)
  }
  let result = ""
  let cursor = 0
  for (const tag of tags) {
    result += text.slice(cursor, tag.start)
    result += tag.matched ? tag.value : tag.value.replaceAll("<", "&lt;").replaceAll(">", "&gt;")
    cursor = tag.end
  }
  return result + text.slice(cursor)
}

export function prepareMarkdown(text: string, literalBackslashes = false) {
  return escapeUnbalancedHtml(literalBackslashes ? preserveLiteralBackslashes(text) : fixEscapedEmphasis(text))
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
    button.setAttribute("aria-label", t("drift.markdown.copyCode"))
    button.title = t("drift.markdown.copyCode")
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
      button.setAttribute("aria-label", t("drift.markdown.codeCopied"))
      button.title = t("drift.markdown.copied")
      setTimeout(() => {
        button.innerHTML = copyIcon
        button.setAttribute("aria-label", t("drift.markdown.copyCode"))
        button.title = t("drift.markdown.copyCode")
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
    const shikiTheme = syntaxTheme() as BundledTheme
    const current = ++request
    setHtml("")
    void highlightedCode(code, lang, shikiTheme)
      .then((output) => current === request && setHtml(output))
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

export function Markdown(props: { text: string; done?: boolean; literalBackslashes?: boolean }) {
  let root!: HTMLDivElement
  let request = 0
  const html = createMemo(() =>
    DOMPurify.sanitize(marked.parse(prepareMarkdown(props.text, props.literalBackslashes), { async: false })),
  )
  createEffect(() => {
    const source = html()
    const theme = syntaxTheme() as BundledTheme
    const current = ++request
    root.innerHTML = source
    decorateCodeBlocks(root)
    if (props.done)
      void highlightBlocks(root, theme, () => current === request).then(() => {
        if (current === request) decorateCodeBlocks(root)
      })
  })
  return <div ref={root} class="md" onClick={markdownClick} />
}
