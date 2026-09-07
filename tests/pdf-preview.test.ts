import { afterEach, expect, mock, test } from "bun:test"
import * as ts from "typescript"
import * as solid from "solid-js/dist/solid.js"

const source = await Bun.file(new URL("../src/ui/pdf-preview.tsx", import.meta.url)).text()
const parsed = ts.createSourceFile("pdf-preview.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX)
const component = parsed.statements.find((node): node is ts.FunctionDeclaration => ts.isFunctionDeclaration(node) && node.name?.text === "PdfPreview")!
const body = component.body!.statements
const jsxReturn = body[body.length - 1]
expect(ts.isReturnStatement(jsxReturn)).toBe(true)

// Run the unchanged lifecycle with real Solid browser reactivity. Only the JSX return
// is replaced with ref wiring and state access, so these tests need no DOM dependency.
const executable = parsed.statements.filter((node) => !ts.isImportDeclaration(node)).map((node) => {
  if (node !== component) return node.getText(parsed)
  return `function PdfPreview(props: { data: Uint8Array; initialPage?: number }) {
    ${body.slice(0, -1).map((statement) => statement.getText(parsed)).join("\n")}
    container = dom.container;
    pageHost = dom.pageHost;
    return { pdf, pageNumber, zoom, busy, error, changePage, changeZoom };
  }`
}).join("\n")
const compile = new Function("solid", "getDocument", "window", "ResizeObserver", "dom", `
  const { createEffect, createSignal, onCleanup, onMount, untrack } = solid;
  const GlobalWorkerOptions = {};
  const workerSrc = "bundled-worker.mjs";
  const t = (key) => key;
  ${ts.transpileModule(executable, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None } }).outputText}
  return PdfPreview;
`)

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}

type Canvas = {
  width: number
  height: number
  style: Record<string, string>
  getContext: ReturnType<typeof mock>
  remove: ReturnType<typeof mock>
}
type Render = ReturnType<typeof deferred<void>> & {
  canvas: Canvas
  viewport: { width: number; height: number }
  transform: number[]
  cancel: ReturnType<typeof mock>
}
type Preview = {
  pdf: () => unknown
  pageNumber: () => number
  zoom: () => number
  busy: () => boolean
  error: () => string | undefined
  changePage: (value: number) => void
  changeZoom: (value: number) => void
}
const disposals: (() => void)[] = []
afterEach(() => { for (const dispose of disposals.splice(0)) dispose() })
async function flush() { for (let i = 0; i < 8; i++) await Promise.resolve() }

function setup(initialPage?: number, initialData = new Uint8Array([1, 2, 3])) {
  const [data, setData] = solid.createSignal(initialData)
  const [initial, setInitialPage] = solid.createSignal(initialPage)
  const loads: (ReturnType<typeof deferred<ReturnType<typeof makePdf>>> & { destroy: ReturnType<typeof mock> })[] = []
  const getDocument = mock((_options: Record<string, unknown>) => {
    const load = { ...deferred<ReturnType<typeof makePdf>>(), destroy: mock(async () => {}) }
    loads.push(load)
    return load
  })
  const renders: Render[] = []
  const canvases: Canvas[] = []
  const dom = {
    container: { clientWidth: 632, scrollTop: 100, scrollLeft: 100 },
    pageHost: {
      children: [] as Canvas[],
      replaceChildren: mock((canvas: Canvas) => { dom.pageHost.children = [canvas] }),
    },
  }
  const window = {
    devicePixelRatio: 2,
    document: {
      createElement: mock((tag: string) => {
        expect(tag).toBe("canvas")
        const canvas: Canvas = {
          width: 0, height: 0, style: {}, getContext: mock(() => ({})),
          remove: mock(() => { dom.pageHost.children = dom.pageHost.children.filter((child) => child !== canvas) }),
        }
        canvases.push(canvas)
        return canvas
      }),
    },
  }
  let measure!: () => void
  const disconnect = mock(() => {})
  class ResizeObserver {
    constructor(callback: () => void) { measure = callback }
    observe(element: unknown) { expect(element).toBe(dom.container) }
    disconnect = disconnect
  }
  function makePdf(numPages = 5, baseWidth = 1200, baseHeight = 1600) {
    const page = {
      getViewport: ({ scale }: { scale: number }) => ({ width: baseWidth * scale, height: baseHeight * scale }),
      render: mock((options: Pick<Render, "canvas" | "viewport" | "transform">) => {
        const result: Render = {
          ...deferred<void>(), ...options,
          cancel: mock(() => result.reject(Object.assign(new Error("cancelled"), { name: "RenderingCancelledException" }))),
        }
        renders.push(result)
        return result
      }),
    }
    return { numPages, getPage: mock(async (_number: number) => page), page }
  }
  const Component = compile(solid, getDocument, window, ResizeObserver, dom)
  let state!: Preview
  let dispose!: () => void
  solid.createRoot((cleanup: () => void) => {
    dispose = cleanup
    state = Component({ get data() { return data() }, get initialPage() { return initial() } })
  })
  disposals.push(dispose)
  return { state, dispose, setData, setInitialPage, loads, getDocument, renders, canvases, dom, window, disconnect, resize: () => measure(), makePdf }
}

test("loads only a copy of the provided byte range and disables external asset fetching", async () => {
  const backing = new Uint8Array([9, 1, 2, 3, 8])
  const input = backing.subarray(1, 4)
  const view = setup(undefined, input)
  const options = view.getDocument.mock.calls[0][0]
  expect(options.data).toEqual(new Uint8Array([1, 2, 3]))
  expect(options.data).not.toBe(input)
  expect((options.data as Uint8Array).buffer).not.toBe(input.buffer)
  structuredClone(options.data, { transfer: [(options.data as Uint8Array).buffer] })
  expect([...backing]).toEqual([9, 1, 2, 3, 8])
  expect(options).toMatchObject({ useWorkerFetch: false, useWasm: false, useSystemFonts: true, enableXfa: false })
  for (const key of ["url", "docBaseUrl", "cMapUrl", "iccUrl", "standardFontDataUrl", "wasmUrl", "isEvalSupported"]) {
    expect(options).not.toHaveProperty(key)
  }
  view.dispose()
  view.loads[0].resolve(view.makePdf())
  await flush()
})

test("clamps the initial page, fits width, and preserves initialPage as an initial-only hint", async () => {
  for (const [initial, expected] of [[undefined, 1], [NaN, 1], [Infinity, 1], [-2, 1], [2.9, 2], [999, 5]] as const) {
    const view = setup(initial)
    const pdf = view.makePdf()
    view.loads[0].resolve(pdf)
    await flush()
    expect(view.state.pageNumber()).toBe(expected)
    expect(pdf.getPage.mock.calls).toEqual([[expected]])
    expect(view.renders[0].viewport).toEqual({ width: 600, height: 800 })
    expect(view.canvases[0]).toMatchObject({ width: 1200, height: 1600 })
    view.renders[0].resolve()
    await flush()
    expect(view.dom.pageHost.children).toEqual([view.canvases[0]])
    expect(view.state.busy()).toBe(false)
    view.setInitialPage(3)
    expect(view.loads).toHaveLength(1)
    expect(view.state.pageNumber()).toBe(expected)
    view.state.changePage(NaN)
    expect(view.state.pageNumber()).toBe(expected)
    view.dispose()
    expect(view.canvases[0].width).toBe(0)
    expect(view.loads[0].destroy).toHaveBeenCalledTimes(1)
  }
})

test("bounds zoom and canvas memory, including extreme aspect ratios and high DPI", async () => {
  for (const height of [1600, 1e9, 0.0001]) {
    const view = setup()
    view.window.devicePixelRatio = 8
    view.loads[0].resolve(view.makePdf(5, 1200, height))
    await flush()
    view.state.changeZoom(100)
    await flush()
    expect(view.state.zoom()).toBe(4)
    expect(view.renders.at(-1)!.viewport.width).toBe(2400)
    for (const canvas of view.canvases) {
      expect(canvas.width * canvas.height).toBeLessThanOrEqual(8_000_000)
      expect(Math.max(canvas.width, canvas.height)).toBeLessThanOrEqual(8192)
    }
    view.state.changeZoom(0)
    await flush()
    expect(view.state.zoom()).toBe(0.25)
    view.dispose()
  }
})

test("rapid page and zoom changes cancel old tasks without sharing canvases or publishing late results", async () => {
  const view = setup()
  view.loads[0].resolve(view.makePdf())
  await flush()
  const first = view.renders[0]
  // Model cancellation settling after a later render has already completed.
  first.cancel.mockImplementation(() => {})
  view.state.changePage(2)
  await flush()
  const second = view.renders[1]
  view.state.changeZoom(2)
  await flush()
  const latest = view.renders[2]
  expect(first.cancel).toHaveBeenCalledTimes(1)
  expect(second.cancel).toHaveBeenCalledTimes(1)
  expect(new Set(view.renders.map((render) => render.canvas)).size).toBe(3)
  expect(first.canvas.width).toBeGreaterThan(0)
  expect(second.canvas.width).toBe(0)
  latest.resolve()
  await flush()
  first.resolve()
  await flush()
  expect(view.dom.pageHost.children).toEqual([latest.canvas])
  expect(view.dom.pageHost.replaceChildren).toHaveBeenCalledTimes(1)
  expect(first.canvas.width).toBe(0)
  expect(view.state.error()).toBeUndefined()
  expect(view.state.busy()).toBe(false)
})

test("stale getPage resolutions and rejections never start a render or overwrite state", async () => {
  for (const reject of [false, true]) {
    const view = setup()
    const pdf = view.makePdf()
    const oldPage = deferred<typeof pdf.page>()
    pdf.getPage.mockImplementationOnce(() => oldPage.promise)
    view.loads[0].resolve(pdf)
    await flush()
    view.state.changePage(99)
    await flush()
    expect(view.state.pageNumber()).toBe(5)
    expect(view.renders).toHaveLength(1)
    view.renders[0].resolve()
    await flush()
    if (reject) oldPage.reject(new Error("late failure"))
    else oldPage.resolve(pdf.page)
    await flush()
    expect(view.renders).toHaveLength(1)
    expect(view.state.error()).toBeUndefined()
    expect(view.state.busy()).toBe(false)
    view.dispose()
  }
})

test("replacing data and unmounting destroy pending loads, ignoring late successes and failures", async () => {
  const view = setup()
  const oldPdf = view.makePdf()
  view.setData(new Uint8Array([4, 5]))
  expect(view.loads[0].destroy).toHaveBeenCalledTimes(1)
  view.loads[0].resolve(oldPdf)
  await flush()
  expect(oldPdf.getPage).not.toHaveBeenCalled()
  expect(view.state.pdf()).toBeUndefined()
  view.dispose()
  expect(view.loads[1].destroy).toHaveBeenCalledTimes(1)
  view.loads[1].reject(new Error("worker stopped"))
  await flush()
  expect(view.loads[1].destroy).toHaveBeenCalledTimes(1)
  expect(view.state.error()).toBeUndefined()
  expect(view.disconnect).toHaveBeenCalledTimes(1)
})

test("unmount cancels a pending render and releases its canvas only after settlement", async () => {
  const view = setup()
  view.loads[0].resolve(view.makePdf())
  await flush()
  const task = view.renders[0]
  task.cancel.mockImplementation(() => {})
  view.dispose()
  expect(task.cancel).toHaveBeenCalledTimes(1)
  expect(task.canvas.width).toBeGreaterThan(0)
  expect(view.loads[0].destroy).toHaveBeenCalledTimes(1)
  task.resolve()
  await flush()
  expect(task.canvas.width).toBe(0)
  expect(view.dom.pageHost.children).toEqual([])
})

test("load and password failures use translated errors and destroy even if destruction rejects", async () => {
  for (const name of ["InvalidPDFException", "PasswordException"]) {
    const view = setup()
    view.loads[0].destroy.mockImplementation(async () => { throw new Error("worker failure") })
    view.loads[0].reject(Object.assign(new Error("untrusted PDF message"), { name }))
    await flush()
    expect(view.state.error()).toBe(name === "PasswordException" ? "drift.preview.pdfPassword" : "drift.preview.error")
    expect(view.state.busy()).toBe(false)
    expect(view.loads[0].destroy).toHaveBeenCalledTimes(1)
    view.dispose()
    expect(view.loads[0].destroy).toHaveBeenCalledTimes(1)
  }
})

test("page, canvas and render failures release the document and report a generic error", async () => {
  for (const failure of ["page", "dimensions", "canvas", "render", "unexpected cancellation"]) {
    const view = setup()
    const pdf = view.makePdf(5, failure === "dimensions" ? 0 : 1200)
    if (failure === "page") pdf.getPage.mockImplementation(async () => { throw new Error("bad page") })
    if (failure === "canvas") {
      const create = view.window.document.createElement.getMockImplementation()!
      view.window.document.createElement.mockImplementation((tag) => {
        const canvas = create(tag)
        canvas.getContext.mockImplementation(() => null)
        return canvas
      })
    }
    view.loads[0].resolve(pdf)
    await flush()
    if (failure === "render") view.renders[0].reject(new Error("bad render"))
    if (failure === "unexpected cancellation") view.renders[0].cancel()
    await flush()
    expect(view.state.error()).toBe("drift.preview.error")
    expect(view.state.busy()).toBe(false)
    expect(view.state.pdf()).toBeUndefined()
    expect(view.loads[0].destroy).toHaveBeenCalledTimes(1)
    expect(view.canvases.every((canvas) => canvas.width === 0)).toBe(true)
    view.dispose()
  }
})

test("hidden containers wait for a width, resize keeps relative zoom, and new data resets zoom", async () => {
  const view = setup()
  view.dom.container.clientWidth = 0
  view.resize()
  view.loads[0].resolve(view.makePdf())
  await flush()
  expect(view.renders).toHaveLength(0)
  view.dom.container.clientWidth = 332
  view.resize()
  await flush()
  expect(view.renders[0].viewport.width).toBe(300)
  view.state.changeZoom(2)
  await flush()
  view.dom.container.clientWidth = 432
  view.resize()
  await flush()
  expect(view.renders.at(-1)!.viewport.width).toBe(800)
  view.setData(new Uint8Array([6]))
  expect(view.state.zoom()).toBe(1)
  expect(view.loads[0].destroy).toHaveBeenCalledTimes(1)
})

test("markup has translated, labelled native controls and no embedded viewer or action layer", () => {
  const markup = jsxReturn.getText(parsed)
  for (const key of ["loading", "previous", "next", "page", "zoomIn", "zoomOut", "pdfPassword", "error"]) {
    expect(source).toContain(`"drift.preview.${key}"`)
  }
  expect(markup.match(/<button type="button"/g)).toHaveLength(4)
  expect(markup.match(/aria-label=\{t\("drift.preview./g)).toHaveLength(5)
  expect(markup).toContain('type="number"')
  expect(markup).toContain('role="alert"')
  expect(markup).toContain('aria-busy={busy()}')
  expect(source).not.toMatch(/<(iframe|object|embed)\b|\b(getJSActions|getAttachments|getOpenAction|AnnotationLayer|XfaLayer|fetch)\s*\(/)
})
