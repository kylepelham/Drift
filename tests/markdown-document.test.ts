import { afterEach, expect, mock, test } from "bun:test"
import * as ts from "typescript"
import * as solid from "solid-js/dist/solid.js"
import { previewParentDirectory } from "../src/file-preview"
import { filePreviewLimits, filePreviewMime, filePreviewType } from "../src/file-preview-types"
import { classifyMarkdownLink } from "../src/ui/markdown-links"

const source = await Bun.file(new URL("../src/ui/markdown-document.tsx", import.meta.url)).text()
const helperSource = await Bun.file(new URL("../src/ui/markdown-images.ts", import.meta.url)).text()
const markdown = await Bun.file(new URL("../src/ui/markdown.tsx", import.meta.url)).text()
const helper = ts.createSourceFile("markdown-images.ts", helperSource, ts.ScriptTarget.Latest, true)
const parsed = ts.createSourceFile("document.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX)
const component = parsed.statements.find((node): node is ts.FunctionDeclaration => ts.isFunctionDeclaration(node) && node.name?.text === "MarkdownDocument")!
const statements = component.body!.statements
const executable = `function MarkdownDocument(props) {
  ${statements.slice(0, -1).map((node) => node.getText(parsed)).join("\n")}
  root = dom;
}`
const compile = new Function("solid", "dom", "readFilePreview", "previewParentDirectory", "filePreviewLimits", "filePreviewMime", "filePreviewType", "classifyMarkdownLink", "shouldPreviewFile", "MutationObserver", "URL", "requestAnimationFrame", "cancelAnimationFrame", "openLightbox", `
  const { createEffect, onCleanup } = solid;
  ${ts.transpileModule(helper.statements.filter((node) => !ts.isImportDeclaration(node)).map((node) => node.getText(helper).replace(/^export /, "")).join("\n"), { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None } }).outputText}
  ${ts.transpileModule(executable, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None } }).outputText}
  return { MarkdownDocument, observeMarkdownImages };
`)
const disposals: (() => void)[] = []
afterEach(() => { for (const dispose of disposals.splice(0)) dispose() })
async function flush() { for (let index = 0; index < 12; index++) await Promise.resolve() }

function image(raw: string) {
  const attributes = new Map([["data-document-image", raw], ["alt", "diagram"]])
  return {
    title: "", onload: null as (() => void) | null, onerror: null as (() => void) | null,
    complete: true, naturalWidth: 1, linked: false,
    closest(selector: string): unknown { return selector === "img" ? this : this.linked ? {} : null },
    focus: mock((_options: unknown) => {}),
    get src() { return attributes.get("src") ?? "" },
    set src(value: string) { attributes.set("src", value) },
    getAttribute: (name: string) => attributes.get(name) ?? null,
    setAttribute: (name: string, value: string) => { attributes.set(name, value) },
    removeAttribute: mock((name: string) => { attributes.delete(name) }),
  }
}

function setup(raw: string[], options: { enabled?: boolean; hash?: string; size?: number; pending?: boolean; input?: { parent?: string; directory?: string; enabled: boolean; hash?: string } } = {}) {
  const images = raw.map(image)
  const heading = { id: "heading-1", scrollIntoView: mock(() => {}) }
  const listeners = new Map<string, Set<() => void>>()
  const ownerDocument = {
    addEventListener: mock((type: string, callback: () => void, options: unknown) => {
      expect(options).toEqual({ capture: true, passive: true })
      if (!listeners.has(type)) listeners.set(type, new Set())
      listeners.get(type)!.add(callback)
    }),
    removeEventListener: mock((type: string, callback: () => void, capture: boolean) => {
      expect(capture).toBe(true)
      listeners.get(type)?.delete(callback)
    }),
  }
  let nextFrame = 0
  const frames = new Map<number, () => void>()
  const requestFrame = mock((callback: () => void) => { frames.set(++nextFrame, callback); return nextFrame })
  const cancelFrame = mock((id: number) => { frames.delete(id) })
  const frame = () => {
    const callbacks = [...frames.values()]
    frames.clear()
    for (const callback of callbacks) callback()
  }
  const events = new Map<string, (event: unknown) => void>()
  const open = mock((_image: { url: string; blob?: Blob; filename?: string; mime?: string }) => {})
  const dom = {
    images,
    ownerDocument,
    querySelectorAll: (selector: string) => selector === "[id]" ? [heading] : selector === "img" ? dom.images : dom.images.filter((item) => item.getAttribute("data-document-image") !== null),
    contains: (item: unknown) => dom.images.includes(item as typeof images[number]),
    addEventListener: (type: string, callback: (event: unknown) => void) => events.set(type, callback),
    removeEventListener: (type: string) => events.delete(type),
  }
  const loads: { resolve: (value: { kind: string; bytes: Uint8Array }) => void; reject: (error: Error) => void }[] = []
  const read = mock((_request: { path: string; directory: string }) => new Promise((resolve, reject) => {
    loads.push({ resolve, reject })
    if (!options.pending) resolve({ kind: "image", bytes: new Uint8Array(options.size ?? 3) })
  }))
  let notify!: () => void
  const observe = mock(() => {})
  const disconnect = mock(() => {})
  class Observer {
    constructor(callback: () => void) { notify = callback }
    observe = observe
    disconnect = disconnect
  }
  let sequence = 0
  const urls = { createObjectURL: mock((_blob: Blob) => `blob:owned-${++sequence}`), revokeObjectURL: mock((_url: string) => {}) }
  const [text, setText] = solid.createSignal("initial")
  const [hash, setHash] = solid.createSignal(options.hash)
  const [directory, setDirectory] = solid.createSignal("C:/work")
  const { MarkdownDocument, observeMarkdownImages } = compile(solid, dom, read, previewParentDirectory, filePreviewLimits, filePreviewMime, filePreviewType, classifyMarkdownLink, () => options.enabled !== false, Observer, urls, requestFrame, cancelFrame, open)
  let dispose!: () => void
  solid.createRoot((cleanup: () => void) => {
    dispose = cleanup
    if (options.input) solid.onCleanup(observeMarkdownImages(dom, options.input))
    else MarkdownDocument({ get text() { return text() }, path: "C:/work/docs/notes.md", get directory() { return directory() }, get hash() { return hash() } })
  })
  disposals.push(dispose)
  return { images, dom, read, loads, urls, observe, disconnect, heading, setText, setHash, setDirectory, dispose, frames, frame, requestFrame, cancelFrame, listeners, open, events,
    activate: (options: Record<string, unknown> = {}) => {
      const event = { type: "click", button: 0, target: dom.images[0], preventDefault: mock(() => {}), stopPropagation: mock(() => {}), ...options }
      events.get(event.type)?.(event)
      return event
    },
    interact: (type: string) => { for (const callback of listeners.get(type) ?? []) callback() }, notify: () => notify() }
}

test("wrapper passes the document parent and original workspace root to Markdown", () => {
  const markup = statements.at(-1)!.getText(parsed)
  expect(markup).toContain("directory={previewParentDirectory(props.path)}")
  expect(markup).toContain("workspaceDirectory={props.directory} documentPreview done")
  expect(markdown).toContain("props.documentPreview ? props.text : prepareMarkdown")
  expect(markdown).toContain("if (documentPreview) return sanitizeMarkdownDocumentHtml(html)")
})

test("only local images reach the bounded reader, always retaining the workspace root", async () => {
  const view = setup(["./diagram.png", "../../outside.png", "file:///C:/work/logo.svg", "https://example.com/x.png", "//example.com/x.png", "data:image/png;base64,eA==", "blob:forged", "file://server/share/x.png", "\\\\server\\share\\x.png", "file:///C:/work/run.cmd", "#heading"])
  await flush()
  expect(view.read.mock.calls).toEqual([
    [{ path: "C:/work/docs/diagram.png", directory: "C:/work" }],
    [{ path: "C:/outside.png", directory: "C:/work" }],
    [{ path: "C:/work/logo.svg", directory: "C:/work" }],
  ])
  expect(view.images.slice(0, 3).map((item) => item.src)).toEqual(["blob:owned-1", "blob:owned-2", "blob:owned-3"])
  expect(view.images.slice(3).every((item) => !item.src && item.title)).toBe(true)
  expect(view.urls.createObjectURL.mock.calls[2][0].type).toBe("image/svg+xml")
})

test("image preference disables automatic reads without opening anything", async () => {
  const view = setup(["image.png"], { enabled: false })
  await flush()
  expect(view.read).not.toHaveBeenCalled()
  expect(view.images[0].title).not.toBe("")
  expect(view.images[0].getAttribute("alt")).toBe("diagram")
})

test.each([{}, { type: "keydown", key: "Enter" }, { type: "keydown", key: " " }])("local images open the lightbox with owned bytes and keyboard access: %j", async (event) => {
  const view = setup(["image.png"])
  await flush()
  const target = view.images[0]
  expect(target.getAttribute("role")).toBe("button")
  expect(target.getAttribute("tabindex")).toBe("0")
  expect(target.getAttribute("class")).toContain("cursor-zoom-in")
  const click = view.activate(event)
  expect(click.preventDefault).toHaveBeenCalled()
  expect(click.stopPropagation).toHaveBeenCalled()
  expect(target.focus).toHaveBeenCalledWith({ preventScroll: true })
  expect(view.open).toHaveBeenCalledWith({ url: "blob:owned-1", blob: view.urls.createObjectURL.mock.calls[0][0], filename: "diagram", mime: "image/png" })
  expect(view.read).toHaveBeenCalledTimes(1)
  const late = view.events.get("click")!
  view.dispose()
  late(click)
  expect(view.open).toHaveBeenCalledTimes(1)
  expect(view.events.size).toBe(0)
  expect(target.getAttribute("role")).toBeNull()
  expect(target.getAttribute("tabindex")).toBeNull()
  expect(target.getAttribute("class")).toBeNull()
})

test("linked, unavailable, detached, modified, and repeated image clicks do not open the lightbox", async () => {
  const view = setup(["image.png"])
  await flush()
  for (const options of [{ button: 1 }, { ctrlKey: true }, { metaKey: true }, { shiftKey: true }, { altKey: true }, { defaultPrevented: true }, { type: "keydown", key: "Escape" }, { type: "keydown", key: "Enter", repeat: true }]) {
    expect(view.activate(options).preventDefault).not.toHaveBeenCalled()
  }
  const target = view.images[0]
  target.linked = true
  view.activate()
  target.linked = false
  target.naturalWidth = 0
  view.activate()
  target.naturalWidth = 1
  target.complete = false
  view.activate()
  target.complete = true
  view.dom.images = []
  view.activate({ target })
  expect(view.open).not.toHaveBeenCalled()
  view.notify()
  expect(target.getAttribute("role")).toBeNull()
  const disabled = setup(["image.png"], { enabled: false })
  await flush()
  disabled.activate()
  expect(disabled.open).not.toHaveBeenCalled()
})

test("external and data images open without file reads, while linked images retain navigation", async () => {
  const view = setup([])
  for (const url of ["https://example.com/image.png", "data:image/png;base64,eA=="]) {
    const target = image("")
    target.removeAttribute("data-document-image")
    target.src = url
    view.dom.images = [target]
    view.notify()
    view.activate()
    expect(view.open.mock.calls.at(-1)?.[0]).toEqual({ url, filename: "diagram", mime: undefined })
    expect(view.open.mock.calls.at(-1)?.[0].blob).toBeUndefined()
  }
  const target = image("linked.png")
  target.linked = true
  view.dom.images = [target]
  view.notify()
  await flush()
  view.activate()
  expect(target.getAttribute("role")).toBeNull()
  expect(view.open).toHaveBeenCalledTimes(2)
  expect(view.read).toHaveBeenCalledTimes(1)
})

test.each([
  { enabled: true, directory: "C:/work" },
  { enabled: true, parent: "C:/work/docs" },
  { enabled: true, parent: "", directory: "C:/work" },
  { enabled: true, parent: "C:/work/docs", directory: "" },
])("missing directories never read even absolute image paths: %j", async (input) => {
  const view = setup(["image.png", "C:/work/image.png", "file:///C:/work/image.png"], { input })
  await flush()
  expect(view.read).not.toHaveBeenCalled()
  expect(view.urls.createObjectURL).not.toHaveBeenCalled()
  expect(view.images.every((item) => !item.src && item.title && item.getAttribute("alt") === "diagram")).toBe(true)
})

test("reads are sequential, capped at 12, and mutations cannot re-read existing images", async () => {
  const view = setup(Array.from({ length: 15 }, (_, index) => `${index}.png`), { pending: true })
  await flush()
  expect(view.read).toHaveBeenCalledTimes(1)
  view.notify()
  expect(view.read).toHaveBeenCalledTimes(1)
  for (let index = 0; index < 12; index++) {
    view.loads[index].resolve({ kind: "image", bytes: new Uint8Array([1]) })
    await flush()
  }
  expect(view.read).toHaveBeenCalledTimes(12)
  for (let index = 0; index < 3; index++) view.notify()
  await flush()
  expect(view.read).toHaveBeenCalledTimes(12)
  expect(view.images[12].title).toContain("limit")
  expect(view.observe.mock.calls[0]).toEqual([view.dom, { childList: true, subtree: true }])
})

test("total read allocation stays within 20 MiB, including failures", async () => {
  const view = setup(["1.png", "2.png", "3.png"], { size: filePreviewLimits.image })
  await flush()
  expect(view.read).toHaveBeenCalledTimes(2)
  expect(view.images[2].title).toContain("limit")
  const failures = setup(["1.png", "2.png", "3.png"], { pending: true })
  await flush()
  failures.loads[0].reject(new Error("outside workspace"))
  await flush()
  failures.loads[1].reject(new Error("symlink escape"))
  await flush()
  expect(failures.read).toHaveBeenCalledTimes(2)
  expect(failures.urls.createObjectURL).not.toHaveBeenCalled()
  expect(failures.images.every((item) => item.title && !item.src)).toBe(true)
})

test("unmount rejects late loads and revokes every published object URL", async () => {
  const view = setup(["1.png", "2.png"], { pending: true })
  await flush()
  view.loads[0].resolve({ kind: "image", bytes: new Uint8Array([1]) })
  await flush()
  view.dispose()
  view.loads[1].resolve({ kind: "image", bytes: new Uint8Array([2]) })
  await flush()
  expect(view.urls.createObjectURL).toHaveBeenCalledTimes(1)
  expect(view.urls.revokeObjectURL.mock.calls).toEqual([["blob:owned-1"]])
  expect(view.disconnect).toHaveBeenCalledTimes(1)
})

test("replaced DOM nodes are not published, cached URLs survive removal, decode errors keep alt", async () => {
  const view = setup(["1.png"], { pending: true })
  await flush()
  const replacement = image("2.png")
  view.dom.images = [replacement]
  view.notify()
  view.loads[0].resolve({ kind: "image", bytes: new Uint8Array([1]) })
  await flush()
  expect(view.urls.createObjectURL).toHaveBeenCalledTimes(1)
  expect(view.images[0].src).toBe("")
  expect(view.images[0].onload).toBeNull()
  view.loads[1].resolve({ kind: "image", bytes: new Uint8Array([2]) })
  await flush()
  replacement.onerror!()
  expect(replacement.title).toContain("could not")
  expect(replacement.src).toBe("")
  expect(replacement.getAttribute("alt")).toBe("diagram")
  expect(view.urls.revokeObjectURL).not.toHaveBeenCalled()
  view.dispose()
  expect(view.urls.revokeObjectURL.mock.calls).toEqual([["blob:owned-1"], ["blob:owned-2"]])
  const loaded = setup(["1.png"])
  await flush()
  loaded.dom.images = []
  loaded.notify()
  expect(loaded.images[0].src).toBe("")
  expect(loaded.urls.revokeObjectURL).not.toHaveBeenCalled()
  loaded.dispose()
  expect(loaded.urls.revokeObjectURL.mock.calls).toEqual([["blob:owned-1"]])
})

test("repeated replacements and duplicate paths reuse one read and URL without spending the budget", async () => {
  const view = setup(["./image.png"], { pending: true })
  await flush()
  view.dom.images = [image("image.png"), image("file:///C:/work/docs/image.png")]
  view.notify()
  view.loads[0].resolve({ kind: "image", bytes: new Uint8Array(filePreviewLimits.image) })
  await flush()
  expect(view.images[0].src).toBe("")
  expect(view.dom.images.map((item) => item.src)).toEqual(["blob:owned-1", "blob:owned-1"])
  for (let index = 0; index < 15; index++) {
    const previous = view.dom.images
    view.dom.images = [image("./image.png")]
    view.notify()
    await flush()
    expect(previous.every((item) => !item.src && item.onload === null && item.onerror === null)).toBe(true)
    expect(view.dom.images[0].src).toBe("blob:owned-1")
  }
  expect(view.read).toHaveBeenCalledTimes(1)
  expect(view.urls.createObjectURL).toHaveBeenCalledTimes(1)
  expect(view.urls.revokeObjectURL).not.toHaveBeenCalled()
  view.dom.images.push(image("second.png"), image("third.png"))
  view.notify()
  await flush()
  view.loads[1].resolve({ kind: "image", bytes: new Uint8Array(filePreviewLimits.image) })
  await flush()
  expect(view.read).toHaveBeenCalledTimes(2)
  expect(view.dom.images[2].title).toContain("limit")
  view.dispose()
  expect(view.dom.images.every((item) => !item.src && item.onload === null && item.onerror === null)).toBe(true)
  expect(view.urls.revokeObjectURL.mock.calls).toEqual([["blob:owned-1"], ["blob:owned-2"]])
})

test("failed reads and decode failures are cached across replacements", async () => {
  for (const decode of [false, true]) {
    const view = setup(["broken.png"], { pending: true })
    await flush()
    if (decode) view.loads[0].resolve({ kind: "image", bytes: new Uint8Array([1]) })
    else view.loads[0].reject(new Error("outside workspace"))
    await flush()
    if (decode) view.images[0].onerror!()
    for (let index = 0; index < 15; index++) {
      view.dom.images = [image("./broken.png")]
      view.notify()
      await flush()
      expect(view.dom.images[0].src).toBe("")
      expect(view.dom.images[0].title).toContain("could not")
      expect(view.dom.images[0].getAttribute("alt")).toBe("diagram")
    }
    expect(view.read).toHaveBeenCalledTimes(1)
    view.dispose()
    expect(view.urls.revokeObjectURL).toHaveBeenCalledTimes(decode ? 1 : 0)
  }
})

test("workspace changes revoke cached URLs and reject reads from the previous observer", async () => {
  const view = setup(["cached.png", "pending.png"], { pending: true })
  await flush()
  view.loads[0].resolve({ kind: "image", bytes: new Uint8Array([1]) })
  await flush()
  view.setDirectory("C:/other")
  await flush()
  expect(view.images[0].src).toBe("")
  expect(view.images[0].onload).toBeNull()
  expect(view.urls.revokeObjectURL.mock.calls).toEqual([["blob:owned-1"]])
  expect(view.read.mock.calls.at(-1)).toEqual([{ path: "C:/work/docs/cached.png", directory: "C:/other" }])
  view.loads[1].resolve({ kind: "image", bytes: new Uint8Array([2]) })
  await flush()
  expect(view.images[1].src).toBe("")
  expect(view.urls.createObjectURL).toHaveBeenCalledTimes(1)
})

test("hash scroll occurs once after headings exist, and content changes discard pending reads", async () => {
  const view = setup(["old.png"], { pending: true, hash: "heading-1" })
  await flush()
  view.notify()
  expect(view.heading.scrollIntoView).toHaveBeenCalledTimes(1)
  view.dom.images = [image("new.png")]
  view.setText("new document")
  await flush()
  view.loads[0].resolve({ kind: "image", bytes: new Uint8Array([1]) })
  view.loads[1].resolve({ kind: "image", bytes: new Uint8Array([2]) })
  await flush()
  expect(view.urls.createObjectURL).toHaveBeenCalledTimes(1)
  expect(view.dom.images[0].src).toBe("blob:owned-1")
})

test("initial hash re-aligns only after actual image loads, for two settling frames per load", async () => {
  const view = setup(["first.png", "second.png"], { hash: "heading-1" })
  await flush()
  expect(view.heading.scrollIntoView).toHaveBeenCalledTimes(1)
  expect(view.requestFrame).not.toHaveBeenCalled()
  expect(view.images[0].src).toBe("blob:owned-1")
  view.images[0].onload!()
  expect(view.images[0].onload).toBeNull()
  expect(view.heading.scrollIntoView).toHaveBeenCalledTimes(1)
  view.frame()
  view.frame()
  expect(view.heading.scrollIntoView).toHaveBeenCalledTimes(3)
  expect(view.frames.size).toBe(0)
  view.images[1].onload!()
  view.frame()
  view.frame()
  expect(view.heading.scrollIntoView).toHaveBeenCalledTimes(5)
  expect(view.heading.scrollIntoView.mock.calls.at(-1)).toEqual([{ block: "start", behavior: "instant" }])
  view.notify()
  view.frame()
  expect(view.heading.scrollIntoView).toHaveBeenCalledTimes(5)
  expect(view.read).toHaveBeenCalledTimes(2)
})

test.each(["wheel", "touchstart", "pointerdown", "keydown", "click"])("%s permanently cancels queued and future image alignment", async (type) => {
  const view = setup(["first.png", "second.png"], { hash: "heading-1" })
  await flush()
  view.images[0].onload!()
  const lateFrame = [...view.frames.values()][0]
  view.interact(type)
  expect(view.frames.size).toBe(0)
  lateFrame()
  view.images[1].onload!()
  view.notify()
  view.frame()
  expect(view.heading.scrollIntoView).toHaveBeenCalledTimes(1)
  expect(view.requestFrame).toHaveBeenCalledTimes(1)
})

test("input before initial rendering prevents even the initial hash jump", async () => {
  const view = setup(["first.png"], { hash: "heading-1" })
  view.interact("wheel")
  await flush()
  view.images[0].onload!()
  view.frame()
  expect(view.heading.scrollIntoView).not.toHaveBeenCalled()
})

test("no-image baseline and missing hashes do not schedule alignment work", async () => {
  const baseline = setup([], { hash: "heading-1" })
  const noHash = setup(["image.png"])
  await flush()
  noHash.images[0].onload!()
  baseline.notify()
  expect(baseline.heading.scrollIntoView).toHaveBeenCalledTimes(1)
  expect(baseline.requestFrame).not.toHaveBeenCalled()
  expect(noHash.heading.scrollIntoView).not.toHaveBeenCalled()
  expect(noHash.requestFrame).not.toHaveBeenCalled()
  expect(noHash.dom.ownerDocument.addEventListener).not.toHaveBeenCalled()
})

test("image errors settle layout too, but never exceed the shared 24-frame budget", async () => {
  const view = setup(Array.from({ length: 12 }, (_, index) => `${index}.png`), { hash: "heading-1" })
  await flush()
  for (const image of view.images) {
    image.onload!()
    view.frame()
    view.frame()
  }
  expect(view.requestFrame).toHaveBeenCalledTimes(24)
  view.images[0].onerror!()
  view.frame()
  expect(view.requestFrame).toHaveBeenCalledTimes(24)
  const failure = setup(["broken.png"], { hash: "heading-1" })
  await flush()
  failure.images[0].onerror!()
  failure.frame()
  failure.frame()
  expect(failure.heading.scrollIntoView).toHaveBeenCalledTimes(3)
  expect(failure.images[0].onload).toBeNull()
  expect(failure.images[0].onerror).toBeNull()
})

test("unmount and replacement clean up handlers, listeners and frames, ignoring late callbacks", async () => {
  for (const replace of [false, true]) {
    const view = setup(["first.png", "second.png"], { hash: "heading-1" })
    await flush()
    const lateLoad = view.images[1].onload!
    const lateError = view.images[1].onerror!
    view.images[0].onload!()
    const lateFrame = [...view.frames.values()][0]
    if (replace) { view.dom.images = []; view.setText("replacement") }
    else view.dispose()
    const scrolls = view.heading.scrollIntoView.mock.calls.length
    lateLoad()
    lateError()
    lateFrame()
    expect(view.heading.scrollIntoView).toHaveBeenCalledTimes(scrolls)
    expect(view.frames.size).toBe(0)
    expect(view.images.every((image) => image.onload === null && image.onerror === null)).toBe(true)
    expect(view.dom.ownerDocument.removeEventListener).toHaveBeenCalledTimes(5)
    expect([...view.listeners.values()].every((callbacks) => callbacks.size === (replace ? 1 : 0))).toBe(true)
    view.dispose()
    await flush()
  }
})

test("detached image callbacks cannot align and mutation cleanup removes their handlers", async () => {
  const view = setup(["image.png"], { hash: "heading-1" })
  await flush()
  const lateLoad = view.images[0].onload!
  view.dom.images = []
  lateLoad()
  expect(view.requestFrame).not.toHaveBeenCalled()
  view.notify()
  expect(view.images[0].onload).toBeNull()
  expect(view.images[0].onerror).toBeNull()
  lateLoad()
  expect(view.requestFrame).not.toHaveBeenCalled()
})

test("preview sanitizer hooks strip forged markers and generate collision-safe heading IDs", () => {
  const ast = ts.createSourceFile("markdown.tsx", markdown, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX)
  const sanitizer = ast.statements.find((node): node is ts.FunctionDeclaration => ts.isFunctionDeclaration(node) && node.name?.text === "sanitizeMarkdownDocumentHtml")!
  const hooks = new Map<string, (node: any, attribute?: any) => void>()
  let config: any
  class Element {
    nodeName = "IMG"
    namespaceURI = "http://www.w3.org/1999/xhtml"
    attributes = new Map<string, string>()
    getAttribute(name: string) { return this.attributes.get(name) ?? null }
    removeAttribute(name: string) { this.attributes.delete(name) }
    setAttribute(name: string, value: string) { this.attributes.set(name, value) }
  }
  const headings = ["Hello World", "Hello World", "hello-world-1", "!!!", "!!!", "Caf\u00e9"].map((textContent) => ({ textContent, id: "forged" }))
  const purifier = {
    addHook: (name: string, callback: typeof hooks extends Map<string, infer T> ? T : never) => hooks.set(name, callback),
    sanitize: (_html: string, options: unknown) => { config = options; return { querySelectorAll: () => headings } },
  }
  const run = new Function("DOMPurify", "window", "document", "classifyMarkdownLink", `
    const markdownImageAttribute = "data-document-image";
    ${ts.transpileModule(sanitizer.getText(ast).replace(/^export /, ""), { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None } }).outputText}
    return sanitizeMarkdownDocumentHtml;
  `)(() => purifier, { Element }, { createElement: () => ({ content: { append() {} }, innerHTML: "safe" }) }, classifyMarkdownLink)
  run("untrusted")
  expect(headings.map((heading) => heading.id)).toEqual(["hello-world", "hello-world-1", "hello-world-1-1", "section", "section-1", "caf\u00e9"])
  expect(config.ALLOW_DATA_ATTR).toBe(false)
  expect(config.ALLOW_ARIA_ATTR).toBe(false)
  for (const name of ["src", "srcset", "style", "background", "poster", "ping", "id", "name", "data-document-image"])
    expect(config.ALLOWED_ATTR).not.toContain(name)
  for (const name of ["script", "style", "svg", "math", "iframe", "object", "embed", "audio", "video", "source", "link", "meta", "base", "input", "form"])
    expect(config.ALLOWED_TAGS).not.toContain(name)
  for (const raw of [undefined, "https://example.com/x.png", "file://server/share/x.png", "./local.png"]) {
    const node = new Element()
    node.setAttribute("data-document-image", "forged.png")
    if (raw) node.setAttribute("src", raw)
    hooks.get("beforeSanitizeAttributes")!(node)
    expect(node.getAttribute("data-document-image")).toBeNull()
    hooks.get("afterSanitizeAttributes")!(node)
    expect(node.getAttribute("data-document-image")).toBe(raw === "./local.png" ? raw : null)
  }
})

test("chat sanitizer replaces only local image sources with trusted reader markers", () => {
  const ast = ts.createSourceFile("markdown.tsx", markdown, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX)
  const sanitizer = ast.statements.find((node): node is ts.FunctionDeclaration => ts.isFunctionDeclaration(node) && node.name?.text === "sanitizeMarkdownHtml")!
  const hooks = new Map<string, (node: any) => void>()
  class Element {
    constructor(public nodeName = "IMG", public namespaceURI = "http://www.w3.org/1999/xhtml") {}
    attributes = new Map<string, string>()
    getAttribute(name: string) { return this.attributes.get(name) ?? null }
    removeAttribute(name: string) { this.attributes.delete(name) }
    setAttribute(name: string, value: string) { this.attributes.set(name, value) }
  }
  const purifier = {
    addHook: (name: string, callback: (node: any) => void) => hooks.set(name, callback),
    sanitize: mock((html: string) => html),
  }
  const createPurifier = mock(() => purifier)
  const documentSanitizer = mock(() => "document")
  const run = new Function("DOMPurify", "window", "classifyMarkdownLink", "sanitizeMarkdownDocumentHtml", `
    let markdownPurifier;
    const markdownImageAttribute = "data-document-image";
    ${ts.transpileModule(sanitizer.getText(ast).replace(/^export /, ""), { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None } }).outputText}
    return sanitizeMarkdownHtml;
  `)(createPurifier, { Element }, classifyMarkdownLink, documentSanitizer)
  run("first")
  run("streaming update")
  expect(createPurifier).toHaveBeenCalledTimes(1)
  expect(run("preview", true)).toBe("document")
  expect(documentSanitizer).toHaveBeenCalledWith("preview")

  const locals = ["Art/MercySlice/Production/ArtDirection/Concepts01/town_board.jpg", "./a%20b.png", "/workspace/image.png", "C:\\workspace\\image.png", "file:///C:/workspace/image.png"]
  const external = ["https://example.com/image.png", "//example.com/image.png", "data:image/png;base64,aGVsbG8="]
  const blocked = ["", "file://server/share/image.png", "\\\\server\\share\\image.png", "javascript:alert(1)", "https://tauri.localhost/image.png", "image%00.png"]
  for (const raw of [...locals, ...external, ...blocked]) {
    const node = new Element()
    node.setAttribute("src", raw)
    node.setAttribute("srcset", "bypass.png 2x")
    node.setAttribute("data-document-image", "forged.png")
    hooks.get("beforeSanitizeAttributes")!(node)
    expect(node.getAttribute("data-document-image")).toBeNull()
    expect(node.getAttribute("srcset")).toBeNull()
    expect(node.getAttribute("src")).toBe(external.includes(raw) ? raw : null)
    hooks.get("afterSanitizeAttributes")!(node)
    expect(node.getAttribute("data-document-image")).toBe(locals.includes(raw) ? raw : null)
  }
  for (const node of [new Element("SOURCE"), new Element("SPAN"), new Element("IMG", "http://www.w3.org/2000/svg")]) {
    node.setAttribute("src", "image.png")
    node.setAttribute("data-document-image", "forged.png")
    if (node.nodeName === "SOURCE") node.setAttribute("srcset", "bypass.png 2x")
    hooks.get("beforeSanitizeAttributes")!(node)
    hooks.get("afterSanitizeAttributes")!(node)
    expect(node.getAttribute("data-document-image")).toBeNull()
    expect(node.getAttribute("srcset")).toBeNull()
  }
})

test("chat image observer tracks ownership and preferences rather than streaming text", () => {
  const ast = ts.createSourceFile("markdown.tsx", markdown, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX)
  const component = ast.statements.find((node): node is ts.FunctionDeclaration => ts.isFunctionDeclaration(node) && node.name?.text === "Markdown")!
  const effect = component.body!.statements.find((node) => node.getText(ast).includes("observeMarkdownImages(root"))!.getText(ast)
  expect(effect).toContain("if (props.documentPreview) return")
  expect(effect).toContain("void props.responseID")
  expect(effect).toContain("onCleanup(observeMarkdownImages(root")
  expect(effect).toContain("parent: props.directory")
  expect(effect).toContain("directory: props.workspaceDirectory ?? props.directory")
  expect(effect).toContain('enabled: shouldPreviewFile("image.png")')
  expect(effect).not.toMatch(/props\.(text|revision|live|done)/)
})
