import { expect, mock, test } from "bun:test"
import * as ts from "typescript"
import * as solid from "solid-js/dist/solid.js"
import { containImage, fitImage, imageWheelScale, maxImageScale, zoomImageAt } from "../src/ui/image-transform"

const source = await Bun.file(new URL("../src/ui/lightbox.tsx", import.meta.url)).text()
const parsed = ts.createSourceFile("lightbox.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX)
const executable = parsed.statements.filter((node) => !ts.isImportDeclaration(node))
  .map((node) => node.getText(parsed).replace(/^export /, "")).join("\n")
// Keep the component bodies and callbacks intact; JSX becomes inspectable nodes,
// while signals and component disposal use real Solid without a DOM or global mocks.
const compile = new Function("solid", "URL", "activateModal", `
  const { createSignal, onCleanup, onMount } = solid;
  const Show = "Show", Portal = "Portal", IconX = "IconX", ImageViewer = "ImageViewer";
  const t = (key) => key;
  const jsx = (type, props, ...children) => ({ type, props: { ...props, children } });
  ${ts.transpileModule(executable, { fileName: "lightbox.tsx", compilerOptions: {
    target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None, jsx: ts.JsxEmit.React, jsxFactory: "jsx",
  } }).outputText}
  return { openLightbox, image, Lightbox };
`)

function setupLightbox() {
  let sequence = 0
  const urls = {
    createObjectURL: mock((_blob: Blob) => `blob:lightbox-${++sequence}`),
    revokeObjectURL: mock((_url: string) => {}),
  }
  const modalCleanups: ReturnType<typeof mock>[] = []
  const activateModal = mock((_dialog: unknown, _onClose: () => void) => {
    const cleanup = mock(() => {})
    modalCleanups.push(cleanup)
    return cleanup
  })
  const state = compile(solid, urls, activateModal)
  const host = solid.createRoot((dispose: () => void) => {
    const host = state.Lightbox()
    const renderDialog = () => solid.createRoot((dispose: () => void) => {
      const portal = host.props.children[0](state.image())
      const dialog = portal.props.children[0]
      return { node: dialog.type(dialog.props), close: dialog.props.onClose, dispose }
    })
    return { dispose, renderDialog }
  })
  return { ...state, urls, activateModal, modalCleanups, ...host }
}

test("lightbox owns a separate URL for a supplied blob without changing caller metadata", () => {
  const view = setupLightbox()
  try {
    const input = { url: "blob:cached", blob: new Blob(["image"]), filename: "diagram.png", mime: "image/png" }
    view.openLightbox(input)
    expect(view.urls.createObjectURL.mock.calls).toEqual([[input.blob]])
    expect(view.image()).toEqual({ ...input, url: "blob:lightbox-1" })
    expect(input.url).toBe("blob:cached")
    expect(view.urls.revokeObjectURL).not.toHaveBeenCalled()
  } finally {
    view.dispose()
  }
  expect(view.image()).toBeNull()
  expect(view.urls.revokeObjectURL.mock.calls).toEqual([["blob:lightbox-1"]])
})

test("replacement revokes the previous owned URL and old dialog cleanup leaves the new image open", () => {
  const view = setupLightbox()
  const input = { url: "blob:cached", blob: new Blob(["image"]) }
  view.openLightbox(input)
  const oldDialog = view.renderDialog()
  try {
    view.openLightbox(input)
    oldDialog.dispose()
    expect(view.modalCleanups[0]).toHaveBeenCalledTimes(1)
    expect(view.image().url).toBe("blob:lightbox-2")
    expect(view.urls.revokeObjectURL.mock.calls).toEqual([["blob:lightbox-1"]])
    const dialog = view.renderDialog()
    try {
      expect(view.activateModal.mock.calls[1][1]).toBe(dialog.close)
      dialog.close()
      dialog.close()
      expect(view.image()).toBeNull()
    } finally {
      dialog.dispose()
    }
  } finally {
    oldDialog.dispose()
    view.dispose()
  }
  expect(view.urls.revokeObjectURL.mock.calls).toEqual([["blob:lightbox-1"], ["blob:lightbox-2"]])
})

test("caller URLs are never allocated or revoked on replacement, close, or host unmount", () => {
  for (const url of ["https://example.com/image.png", "data:image/png;base64,eA==", "blob:caller-owned"]) {
    const view = setupLightbox()
    try {
      const input = { url, filename: "image.png", mime: "image/png" }
      view.openLightbox(input)
      view.openLightbox(input)
      expect(view.image()).toEqual(input)
      expect(view.image()).not.toBe(input)
      const dialog = view.renderDialog()
      try {
        dialog.close()
        expect(view.image()).toBeNull()
      } finally {
        dialog.dispose()
      }
      view.openLightbox(input)
    } finally {
      view.dispose()
    }
    expect(view.image()).toBeNull()
    expect(view.urls.createObjectURL).not.toHaveBeenCalled()
    expect(view.urls.revokeObjectURL).not.toHaveBeenCalled()
  }
})

test("switching between owned and caller URLs only releases lightbox-owned URLs", () => {
  const view = setupLightbox()
  try {
    view.openLightbox({ url: "blob:cached", blob: new Blob(["first"]) })
    view.openLightbox({ url: "blob:caller-owned" })
    expect(view.image().url).toBe("blob:caller-owned")
    expect(view.urls.revokeObjectURL.mock.calls).toEqual([["blob:lightbox-1"]])
    view.openLightbox({ url: "blob:cached", blob: new Blob(["second"]) })
    expect(view.image().url).toBe("blob:lightbox-2")
    expect(view.urls.revokeObjectURL.mock.calls).toEqual([["blob:lightbox-1"]])
  } finally {
    view.dispose()
  }
  expect(view.image()).toBeNull()
  expect(view.urls.revokeObjectURL.mock.calls).toEqual([["blob:lightbox-1"], ["blob:lightbox-2"]])
})

test("lightbox puts filename, secondary metadata, and close in viewer slots without a second header", () => {
  const view = setupLightbox()
  const filename = `${"long-filename-".repeat(20)}.png`
  view.openLightbox({ url: "data:image/png;base64,eA==", filename, mime: "image/png" })
  const dialog = view.renderDialog()
  try {
    expect(dialog.node.props.children).toHaveLength(1)
    const viewer = dialog.node.props.children[0]
    expect(viewer.type).toBe("ImageViewer")
    expect(viewer.props.onBackgroundClick).toBe(dialog.close)
    const start = viewer.props.toolbarStart
    expect(start.props.children).toEqual([filename])
    expect(start.props.title).toBe(filename)
    expect(start.props.class).toBe("min-w-0 flex-1 truncate text-ink")
    const end = viewer.props.toolbarEnd
    expect(end.props.class).toContain("shrink-0")
    const [mime, size, close] = end.props.children
    expect(mime.props.children[0].props.class).toContain("hidden max-w-32 truncate lg:inline")
    expect(size.props.children[0](() => size.props.when).props.class).toContain("hidden shrink-0 lg:inline")
    expect(close.type).toBe("button")
    expect(close.props["aria-label"]).toBe("common.close")
    expect(close.props.class).toContain("size-8 shrink-0")
    expect(close.props.class).not.toContain("hidden")
    close.props.onClick()
    expect(view.image()).toBeNull()
  } finally {
    dialog.dispose()
    view.dispose()
  }
})

test("slotted viewer toolbar is one row with compact reset and hidden mobile dimensions; default toolbar is unchanged", async () => {
  const source = await Bun.file(new URL("../src/ui/image-viewer.tsx", import.meta.url)).text()
  const ast = ts.createSourceFile("image-viewer.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX)
  const canvas = ast.statements.find((node): node is ts.FunctionDeclaration => ts.isFunctionDeclaration(node) && node.name?.text === "ImageCanvas")!
  const compact = canvas.body!.statements.find((node) => ts.isVariableStatement(node)
    && node.declarationList.declarations.some((declaration) => declaration.name.getText(ast) === "compactToolbar"))!
  let toolbar!: ts.JsxElement
  function visit(node: ts.Node) {
    if (ts.isJsxElement(node) && node.openingElement.attributes.properties.some((attribute) => ts.isJsxAttribute(attribute) && attribute.name.getText(ast) === "data-image-toolbar")) toolbar = node
    ts.forEachChild(node, visit)
  }
  visit(canvas)
  // Execute the actual toolbar JSX and click handlers with fixed image state.
  const render = new Function("props", "reset", "zoom", `
    const jsx = (type, props, ...children) => ({ type, props: { ...props, children } });
    const Show = "Show", IconRestore = "IconRestore", t = (key) => key;
    const natural = () => ({ width: 6000, height: 240 }), view = () => ({ scale: 0.5 });
    ${ts.transpileModule(`${compact.getText(ast)}; return (${toolbar.getText(ast)});`, {
      fileName: "toolbar.tsx", compilerOptions: { target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.React, jsxFactory: "jsx" },
    }).outputText}
  `)
  for (const slotted of [false, true]) {
    const reset = mock(() => {})
    const zoom = mock((_scale: number) => {})
    const props = slotted ? { toolbarStart: "filename", toolbarEnd: "close" } : {}
    const row = render(props, reset, zoom)
    expect(row.props.classList).toEqual({ "flex-wrap justify-center": !slotted, "flex-nowrap whitespace-nowrap": slotted })
    expect(row.props.children[0]).toBe(props.toolbarStart)
    expect(row.props.children.at(-1)).toBe(props.toolbarEnd)
    const [, minus, percent, plus, fit, actual, dimensions] = row.props.children
    expect(percent.props.children).toEqual(["50%"])
    for (const control of [minus, plus, fit, actual]) {
      expect(control.props.class).toContain("shrink-0")
      expect(control.props.disabled).toBe(false)
      control.props.onClick()
    }
    expect(zoom.mock.calls).toEqual([[0.4], [0.625], [1]])
    expect(reset).toHaveBeenCalledTimes(1)
    expect(fit.props["aria-label"]).toBe("drift.lightbox.resetZoom")
    expect(fit.props.title).toBe("drift.lightbox.resetZoom")
    const label = fit.props.children[0]
    expect(label.props.when).toBe(slotted)
    expect(label.props.fallback).toBe("drift.lightbox.resetZoom")
    expect(label.props.children[0].type).toBe("IconRestore")
    expect(label.props.children[0].props.class).toBe("size-4 lg:hidden")
    expect(label.props.children[1].props.class).toBe("hidden lg:inline")
    expect(actual.props["aria-label"]).toBe("drift.lightbox.actualSize")
    expect(dimensions.props.children[0](() => dimensions.props.when).props.classList).toEqual({ "hidden lg:inline": slotted })
  }
})

test("fit handles wide, tall, small, and temporarily collapsed viewports", () => {
  expect(fitImage({ width: 6000, height: 240 }, { width: 632, height: 400 })).toEqual({ scale: 0.1, x: 16, y: 188 })
  expect(fitImage({ width: 240, height: 6000 }, { width: 400, height: 632 })).toEqual({ scale: 0.1, x: 188, y: 16 })
  expect(fitImage({ width: 100, height: 80 }, { width: 632, height: 400 })).toEqual({ scale: 1, x: 266, y: 160 })
  expect(fitImage({ width: 6000, height: 240 }, { width: 0, height: 0 }).scale).toBeGreaterThan(0)
})

test("cursor zoom preserves the exact natural image point through repeated changes", () => {
  const anchor = { x: 234.125, y: 190.875 }
  let view = { x: -15.25, y: 21.5, scale: 0.075 }
  const pixel = { x: (anchor.x - view.x) / view.scale, y: (anchor.y - view.y) / view.scale }
  for (const scale of [0.1, 0.3, 1, 4, 8, 32, 100, maxImageScale, 500, 10, 1, 0.075]) {
    view = zoomImageAt(view, scale, anchor)
    expect((anchor.x - view.x) / view.scale).toBeCloseTo(pixel.x, 8)
    expect((anchor.y - view.y) / view.scale).toBeCloseTo(pixel.y, 8)
  }
  expect(maxImageScale).toBe(1024)
})

test("pan keeps large and small images recoverable without snapping them to the center", () => {
  const viewport = { width: 600, height: 400 }
  const image = { width: 160, height: 96 }
  expect(containImage({ x: 50, y: 80, scale: 1 }, image, viewport)).toEqual({ x: 50, y: 80, scale: 1 })
  expect(containImage({ x: -10000, y: -10000, scale: 1 }, image, viewport)).toEqual({ x: -96, y: -48, scale: 1 })
  expect(containImage({ x: 10000, y: 10000, scale: 1 }, image, viewport)).toEqual({ x: 536, y: 352, scale: 1 })
  expect(containImage({ x: -500, y: -800, scale: 100 }, image, viewport)).toEqual({ x: -500, y: -800, scale: 100 })
})

test("wheel zoom uses delta magnitude and normalizes pixel, line, and page modes", () => {
  expect(imageWheelScale(0, 0, 800)).toBe(1)
  expect(imageWheelScale(-60, 0, 800)).toBeGreaterThan(1)
  expect(imageWheelScale(60, 0, 800)).toBeLessThan(1)
  expect(imageWheelScale(-120, 0, 800)).toBeCloseTo(imageWheelScale(-60, 0, 800) ** 2)
  expect(imageWheelScale(3, 1, 800)).toBe(imageWheelScale(48, 0, 800))
  expect(imageWheelScale(0.25, 2, 800)).toBe(imageWheelScale(200, 0, 800))
  expect(imageWheelScale(-100000, 0, 800)).toBe(Math.E)
})

test("both image hosts use the same transform viewer with ordinary wheel zoom and cleanup", async () => {
  const lightbox = await Bun.file("src/ui/lightbox.tsx").text()
  const preview = await Bun.file("src/ui/file-preview.tsx").text()
  const viewer = await Bun.file("src/ui/image-viewer.tsx").text()
  expect(lightbox).toContain("<ImageViewer")
  expect(preview).toContain("<ImageViewer")
  expect(viewer).toContain('viewport.addEventListener("wheel", wheel, { passive: false })')
  expect(viewer).toContain('viewport.removeEventListener("wheel", wheel)')
  expect(viewer).toContain("observer.disconnect()")
  expect(viewer).not.toContain("if (!event.ctrlKey && !event.metaKey) return")
  expect(viewer).toContain("viewport.setPointerCapture(event.pointerId)")
  expect(viewer).toContain("onPointerCancel={endPointer}")
  expect(viewer).toContain("onLostPointerCapture={endPointer}")
  expect(viewer).toContain('class="absolute left-0 top-0 max-w-none select-none"')
  expect(viewer).toContain("Number.parseFloat(style.width)")
  expect(viewer).toContain("Number.parseFloat(style.height)")
})

test("every locale explains image gestures and keyboard controls", async () => {
  for (const locale of ["en", "ar", "br", "bs", "da", "de", "es", "fr", "ja", "ko", "no", "pl", "ru", "th", "tr", "uk", "zh", "zht"]) {
    const { drift } = await import(`../src/i18n/${locale}`)
    expect(drift["drift.lightbox.controls"]).toBeString()
    expect(drift["drift.lightbox.controls"].length).toBeGreaterThan(0)
  }
})
