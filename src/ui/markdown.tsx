import DOMPurify from "dompurify"
import { marked } from "marked"
import type { BundledLanguage, BundledTheme, SpecialLanguage } from "shiki"
import { createEffect, createMemo, createSignal, For, onCleanup, Show } from "solid-js"
import { shellInvoke } from "../shell"
import { t } from "../state/i18n"
import { syntaxTheme } from "../state/code"

marked.use({ gfm: true, breaks: true })

let shikiModule: Promise<typeof import("shiki")> | undefined
const codeBlocks = new WeakMap<HTMLElement, string>()
// How long the code-block copy button shows its "copied" state.
// NOTE: parts.tsx uses 2000ms for its visually identical shell copy button.
const copiedFeedbackMs = 1600
// Code blocks start highlighting this far outside the viewport so scrolling lands on rendered code.
const chunkPrefetchMargin = "320px 0px"
// Markers the rendered HTML carries so the delegated copy handler can find its block.
const codeBlockAttribute = "[data-code-block]"
const copyButtonAttribute = "[data-copy-code]"
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

  /**
   * Stores an in-flight promise under a provisional size, then corrects that size once it resolves.
   *
   * The real cost is not known until the value exists, but the entry has to be cached immediately
   * so concurrent callers share one computation. It is therefore charged its source size up front
   * and re-measured on resolution. A value larger than the whole budget is returned uncached rather
   * than admitted and immediately evicted. A rejected promise is evicted so the failure is retried.
   */
  set(key: string, sourceSize: number, value: Promise<T>) {
    const existing = this.entries.get(key)
    if (existing) this.remove(key, existing)
    if (sourceSize > this.limit) return value

    const entry: AsyncCacheEntry<T> = { value, size: sourceSize }
    const tracked = value.then(
      (result) => {
        // The identity check matters: this entry may have been evicted or replaced while the
        // promise was pending, in which case its size is no longer part of the running total.
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

// The cache is budgeted in approximate bytes, so every entry is charged its payload length plus a
// rough per-object overhead. These weights only need to be proportional, not exact - they exist so
// that many small entries cannot sit under the budget while consuming real memory.
const perLineOverhead = 16
const perTokenOverhead = 32
const perSourceEntryOverhead = 128
const perHtmlEntryOverhead = 64

function tokenOutputSize(lines: SyntaxToken[][]) {
  let size = lines.length * perLineOverhead
  for (const line of lines) for (const token of line) size += token.content.length + perTokenOverhead
  return size
}

function shikiSourceSize(key: string, code: string) {
  return key.length + code.length + perSourceEntryOverhead
}

const shikiCacheBudget = 2 * 1024 * 1024
const highlightCache = new AsyncSizeCache<string>(shikiCacheBudget, (html) => html.length + perHtmlEntryOverhead)
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

/**
 * Applies `transform` to the prose in `text`, leaving fenced blocks and inline code untouched.
 *
 * Fences are recognized only at line starts (with up to three spaces of indentation), while inline
 * spans use an exact-length closing backtick run, matching CommonMark's delimiter rules.
 */
function mapProseChunks(text: string, transform: (chunk: string) => string) {
  let result = ""
  let proseStart = 0
  let index = 0

  const preserve = (start: number, end: number) => {
    result += transform(text.slice(proseStart, start)) + text.slice(start, end)
    proseStart = end
    index = end
  }

  while (index < text.length) {
    const lineStart = index === 0 || text[index - 1] === "\n"
    if (lineStart) {
      let markerStart = index
      while (markerStart < index + 3 && text[markerStart] === " ") markerStart++
      const marker = text[markerStart]
      if (marker === "`" || marker === "~") {
        let markerEnd = markerStart
        while (text[markerEnd] === marker) markerEnd++
        const markerLength = markerEnd - markerStart
        const openerEnd = text.indexOf("\n", markerEnd)
        const infoEnd = openerEnd < 0 ? text.length : openerEnd
        if (markerLength >= 3 && (marker !== "`" || !text.slice(markerEnd, infoEnd).includes("`"))) {
          let closeStart = openerEnd < 0 ? text.length : openerEnd + 1
          let fenceEnd = text.length
          while (closeStart < text.length) {
            let closeMarkerStart = closeStart
            while (closeMarkerStart < closeStart + 3 && text[closeMarkerStart] === " ") closeMarkerStart++
            let closeMarkerEnd = closeMarkerStart
            while (text[closeMarkerEnd] === marker) closeMarkerEnd++
            const closeLineEnd = text.indexOf("\n", closeMarkerEnd)
            const trailingEnd = closeLineEnd < 0 ? text.length : closeLineEnd
            if (
              closeMarkerEnd - closeMarkerStart >= markerLength &&
              text.slice(closeMarkerEnd, trailingEnd).trim() === ""
            ) {
              fenceEnd = closeLineEnd < 0 ? text.length : closeLineEnd + 1
              break
            }
            const nextLine = text.indexOf("\n", closeStart)
            if (nextLine < 0) break
            closeStart = nextLine + 1
          }
          preserve(index, fenceEnd)
          continue
        }
      }
    }

    if (text[index] === "`") {
      const start = index
      let openerEnd = index
      while (text[openerEnd] === "`") openerEnd++
      const length = openerEnd - index
      let cursor = openerEnd
      while (cursor < text.length) {
        const close = text.indexOf("`", cursor)
        if (close < 0) break
        let closeEnd = close
        while (text[closeEnd] === "`") closeEnd++
        if (closeEnd - close === length) {
          preserve(index, closeEnd)
          break
        }
        cursor = closeEnd
      }
      if (index === start) index = openerEnd
      continue
    }
    index++
  }

  return result + transform(text.slice(proseStart))
}

// Model output like **C:\** never closes its emphasis because \ escapes the delimiter.
// Only path-shaped backslashes (after a drive colon or another segment) get self-escaped.
export function fixEscapedEmphasis(text: string) {
  return mapProseChunks(text, (chunk) =>
    chunk.replace(/(:)\\(?=\*\*?|__?)/g, "$1\\\\").replace(/(\\[\w .()-]+)\\(?=\*\*?|__?)/g, "$1\\\\"),
  )
}

const urlPattern = /https?:\/\/[^\s<>"')\]]+/g
const accidentalEntities: Record<string, string> = { "-": "&#45;", "=": "&#61;", "*": "&#42;", _: "&#95;" }

// Preserve literal prose and pasted markup while retaining deliberate Markdown constructs.
function humanizeProse(text: string) {
  return mapProseChunks(text, (chunk) =>
    chunk.replaceAll("\\", "&#92;").replaceAll("<", "&lt;").split("\n").map(neutralizeProseLine).join("\n"),
  )
}

/** Neutralizes block and emphasis syntax commonly pasted from terminals or prose. */
function neutralizeProseLine(line: string) {
  let result = line.replace(/^(\s{0,3})>/, "$1&gt;").replace(/^(\s{0,3})#(?=#{0,5}(\s|$))/, "$1&#35;")
  if (/^\s{0,3}[-=*_](\s*[-=*_])*\s*$/.test(result)) {
    result = result.replace(/[-=*_]/, (marker) => accidentalEntities[marker])
  }
  let escaped = ""
  let cursor = 0
  for (const match of result.matchAll(urlPattern)) {
    escaped += escapeEmphasis(result.slice(cursor, match.index)) + match[0]
    cursor = match.index + match[0].length
  }
  return escaped + escapeEmphasis(result.slice(cursor))
}

function escapeEmphasis(text: string) {
  return text.replaceAll("*", "&#42;").replaceAll("_", "&#95;").replaceAll("~~", "&#126;&#126;")
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
function escapeUnbalancedHtml(text: string) {
  return mapProseChunks(text, escapeUnbalancedHtmlChunk)
}

/**
 * Escapes HTML tags in one prose chunk that do not form a matched open/close pair.
 *
 * Streamed model output regularly contains a stray `<div>` or a half-written tag. Marked would
 * treat it as real HTML and swallow the rest of the response into that element, so unmatched tags
 * are rendered as literal text instead. Tags that do pair up are left alone so intentional inline
 * HTML keeps working.
 *
 * Pairing is a standard bracket match: openers are pushed on a stack, and a closer scans back for
 * the nearest opener of the same name. Scanning back rather than only checking the top tolerates
 * improperly nested but individually paired tags such as `<b><i></b></i>`.
 */
function escapeUnbalancedHtmlChunk(text: string) {
  const tags = [...text.matchAll(/<!--[^]*?-->|<\/?[A-Za-z][^>\n]*>/g)].map((match) => ({
    start: match.index,
    end: match.index + match[0].length,
    value: match[0],
    name: match[0].match(/^<\/?\s*([A-Za-z][\w:-]*)/)?.[1]?.toLowerCase(),
    closing: /^<\//.test(match[0]),
    matched: false,
  }))

  /** Indices into `tags` of openers still waiting for their closing tag. */
  const stack: number[] = []
  for (let index = 0; index < tags.length; index++) {
    const tag = tags[index]
    // Comments, void elements and self-closing tags never pair, so they are always kept as-is.
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
    // No opener: this closer is stray and stays marked unmatched, so it gets escaped below.
    if (opener < 0) continue
    const openIndex = stack[opener]
    tags[openIndex].matched = true
    tag.matched = true
    stack.splice(opener, 1)
  }

  // Anything left on the stack was opened and never closed, so it keeps matched = false.
  let result = ""
  let cursor = 0
  for (const tag of tags) {
    result += text.slice(cursor, tag.start)
    result += tag.matched ? tag.value : tag.value.replaceAll("<", "&lt;").replaceAll(">", "&gt;")
    cursor = tag.end
  }
  return result + text.slice(cursor)
}

export function prepareMarkdown(text: string, humanAuthored = false) {
  return escapeUnbalancedHtml(humanAuthored ? humanizeProse(text) : fixEscapedEmphasis(text))
}

function openExternalLink(event: MouseEvent) {
  const anchor = (event.target as Element).closest<HTMLAnchorElement>("a[href]")
  if (!anchor) return
  const url = new URL(anchor.href)
  if (url.protocol !== "http:" && url.protocol !== "https:") return
  const invoke = shellInvoke()
  if (!invoke) return
  event.preventDefault()
  void invoke("plugin:opener|open_url", { url: url.href }).catch(() => {})
}

function decorateCodeBlocks(root: HTMLElement) {
  for (const pre of root.querySelectorAll<HTMLElement>("pre")) {
    const code = pre.querySelector("code")?.textContent ?? pre.textContent ?? ""
    const existing = pre.closest<HTMLElement>(codeBlockAttribute)
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
  const button = (event.target as Element).closest<HTMLButtonElement>(copyButtonAttribute)
  if (!button) return openExternalLink(event)
  const wrapper = button.closest<HTMLElement>(codeBlockAttribute)
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
      }, copiedFeedbackMs)
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

function CodeView(props: { code: string; lang: string }) {
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
        { root, rootMargin: chunkPrefetchMargin },
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

export function Markdown(props: { text: string; done?: boolean; humanAuthored?: boolean }) {
  let root!: HTMLDivElement
  let request = 0
  const html = createMemo(() =>
    DOMPurify.sanitize(marked.parse(prepareMarkdown(props.text, props.humanAuthored), { async: false })),
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
  return <div ref={root} class="md" classList={{ "md-user": props.humanAuthored }} onClick={markdownClick} />
}
