import { afterEach, beforeEach, expect, mock, test } from "bun:test"
import { filePreviewTypes, type FilePreviewType } from "../src/file-preview-types"
import { closeFilePreview, previewFile } from "../src/state/file-preview"
import type { FilePreviewPrefs } from "../src/state/file-preview-prefs"

const directory = "C:\\Users\\Kyle\\Desktop\\C++\\Drift"
const rawContract = "EAC/docs/BENIGN_PLATFORM_EXPERIMENT_CONTRACT.md"
const contractPath = "C:/Users/Kyle/Desktop/C++/Drift/EAC/docs/BENIGN_PLATFORM_EXPERIMENT_CONTRACT.md"
const globalNames = ["__TAURI__", "window", "location", "document", "fetch", "localStorage"] as const
let originalGlobals: (PropertyDescriptor | undefined)[]
let openMarkdownLink: typeof import("../src/ui/markdown").openMarkdownLink
let markdownClick: typeof import("../src/ui/markdown").markdownClick
let invoke: ReturnType<typeof mock>
let fetchMock: ReturnType<typeof mock>
let prefs: typeof import("../src/state/file-preview-prefs")
let originalPrefs: FilePreviewPrefs

function setGlobal(name: (typeof globalNames)[number], value: unknown) {
  Object.defineProperty(globalThis, name, { configurable: true, writable: true, value })
}

function runtime(url: string) {
  const location = new URL(url)
  setGlobal("location", location)
  setGlobal("window", { location, open: mock(() => { throw new Error("Unexpected window.open") }) })
  return location
}

beforeEach(async () => {
  originalGlobals = globalNames.map((name) => Object.getOwnPropertyDescriptor(globalThis, name))
  if (!("localStorage" in globalThis)) setGlobal("localStorage", { getItem: () => null, setItem: () => undefined })
  // Import the real handler before installing lightweight browser doubles.
  ;({ openMarkdownLink, markdownClick } = await import("../src/ui/markdown"))
  prefs = await import("../src/state/file-preview-prefs")
  originalPrefs = prefs.normalizeFilePreviewPrefs(prefs.filePreviewPrefs())
  prefs.setFilePreviewMode("none")
  closeFilePreview()
  setGlobal("__TAURI__", undefined)
  runtime("http://localhost:5180/")
  invoke = mock(async (_command: string, _args?: Record<string, unknown>) => ({ positioned: true }))
  fetchMock = mock(async () => { throw new Error("Unexpected network request") })
  setGlobal("fetch", fetchMock)
})

afterEach(() => {
  closeFilePreview()
  prefs.setFilePreviewMode(originalPrefs.mode)
  for (const type of filePreviewTypes) prefs.setFilePreviewType(type, originalPrefs.types[type])
  globalNames.forEach((name, index) => {
    const descriptor = originalGlobals[index]
    if (descriptor) Object.defineProperty(globalThis, name, descriptor)
    else Reflect.deleteProperty(globalThis, name)
  })
})

function native() {
  runtime("http://tauri.localhost/")
  setGlobal("__TAURI__", { core: { invoke } })
}

function click(raw: string, options: { button?: number; defaultPrevented?: boolean } = {}) {
  const anchor = {
    getAttribute: mock((name: string) => {
      expect(name).toBe("href")
      return raw
    }),
    get href(): string {
      throw new Error("The browser-resolved anchor.href must not be read")
    },
  }
  const root = {
    querySelectorAll: mock((_selector: string): { id: string; scrollIntoView: ReturnType<typeof mock> }[] => []),
  }
  const target = {
    closest: mock((selector: string): unknown => {
      expect(selector).toBe("a[href]")
      return anchor
    }),
  }
  const event = {
    type: options.button === 1 ? "auxclick" : "click",
    button: options.button ?? 0,
    defaultPrevented: options.defaultPrevented ?? false,
    target,
    currentTarget: root,
    preventDefault: mock(() => { event.defaultPrevented = true }),
    stopPropagation: mock(() => undefined),
  }
  return { anchor, root, target, event: event as unknown as MouseEvent, preventDefault: event.preventDefault, stopPropagation: event.stopPropagation }
}

function expectStopped(link: ReturnType<typeof click>) {
  expect(link.preventDefault).toHaveBeenCalledTimes(1)
  expect(link.stopPropagation).toHaveBeenCalledTimes(1)
  expect(link.event.defaultPrevented).toBe(true)
}

test("the screenshot's raw relative contract opens in its owning directory, never as a Tauri URL", async () => {
  native()
  const link = click(rawContract)
  Object.defineProperty(link.anchor, "href", { value: `http://tauri.localhost/${rawContract}` })

  await openMarkdownLink(link.event, directory)

  expect(link.anchor.getAttribute).toHaveBeenCalledWith("href")
  expect(invoke.mock.calls).toEqual([["open_file_in_editor", { path: contractPath, line: undefined, column: undefined }]])
  expect(invoke.mock.calls.filter(([command]) => command === "plugin:opener|open_url")).toHaveLength(0)
  expect(fetchMock).not.toHaveBeenCalled()
  expectStopped(link)
})

test("local opening never touches the throwing href getter and uses the supplied session directory", async () => {
  native()
  const link = click(rawContract)

  await openMarkdownLink(link.event, "D:\\other-session")

  expect(invoke.mock.calls).toEqual([["open_file_in_editor", {
    path: "D:/other-session/EAC/docs/BENIGN_PLATFORM_EXPERIMENT_CONTRACT.md", line: undefined, column: undefined,
  }]])
  expectStopped(link)
})

test.each([
  ["C:\\work\\notes.md", "C:/work/notes.md", undefined, undefined],
  ["file:///C:/work/design%20notes.md", "C:/work/design notes.md", undefined, undefined],
  ["src/main.ts#L42", "C:/Users/Kyle/Desktop/C++/Drift/src/main.ts", 42, undefined],
  ["C:\\work\\main.ts#L42C7", "C:/work/main.ts", 42, 7],
  ["file:///C:/work/main.ts#L42-L48", "C:/work/main.ts", 42, undefined],
  ["./payload.cmd", "C:/Users/Kyle/Desktop/C++/Drift/payload.cmd", undefined, undefined],
  ["file:///C:/work/payload.cmd", "C:/work/payload.cmd", undefined, undefined],
])("native file link %s passes path and line/column arguments", async (raw, path, line, column) => {
  native()
  const link = click(raw)

  await openMarkdownLink(link.event, directory)

  expect(invoke.mock.calls).toEqual([["open_file_in_editor", { path, line, column }]])
  expectStopped(link)
})

test("native HTTPS links use only the external URL opener", async () => {
  native()
  const url = "https://example.com/docs?q=drift#intro"
  const link = click(url)

  await openMarkdownLink(link.event, directory)

  expect(invoke.mock.calls).toEqual([["plugin:opener|open_url", { url }]])
  expect(fetchMock).not.toHaveBeenCalled()
  expectStopped(link)
})

test("browser HTTPS links retain default navigation", async () => {
  const link = click("https://example.com/docs")

  await openMarkdownLink(link.event, directory)

  expect(link.preventDefault).not.toHaveBeenCalled()
  expect(link.stopPropagation).not.toHaveBeenCalled()
  expect(link.event.defaultPrevented).toBe(false)
  expect(invoke).not.toHaveBeenCalled()
  expect(fetchMock).not.toHaveBeenCalled()
  expect(window.open).not.toHaveBeenCalled()
})

test("local links without a host backend stop navigation and reject", async () => {
  const link = click(rawContract)

  await expect(openMarkdownLink(link.event, directory)).rejects.toThrow("Opening files requires the Drift host backend")

  expectStopped(link)
  expect(invoke).not.toHaveBeenCalled()
  expect(fetchMock).not.toHaveBeenCalled()
  expect(window.open).not.toHaveBeenCalled()
})

test("relative links without an owning directory stop navigation and reject even with a native backend", async () => {
  native()
  const link = click(rawContract)

  await expect(openMarkdownLink(link.event)).rejects.toThrow("workspace directory is unavailable")

  expectStopped(link)
  expect(invoke).not.toHaveBeenCalled()
  expect(fetchMock).not.toHaveBeenCalled()
})

test.each(["http://tauri.localhost/EAC/docs/contract.md", "tauri://localhost/EAC/docs/contract.md"])(
  "unsupported internal URL %s cannot reach the external opener",
  async (raw) => {
    native()
    const link = click(raw)

    await expect(openMarkdownLink(link.event, directory)).rejects.toThrow("The link is invalid")

    expectStopped(link)
    expect(invoke).not.toHaveBeenCalled()
    expect(fetchMock).not.toHaveBeenCalled()
  },
)

test("hash-only links scroll an exact decoded ID inside the current Markdown, not the browser document", async () => {
  const location = runtime("http://localhost:5180/#/session/current")
  const id = 'section"][data-private]'
  const link = click(`#${encodeURIComponent(id)}`)
  const matching = { id, scrollIntoView: mock(() => undefined) }
  const unrelated = { id: "section", scrollIntoView: mock(() => undefined) }
  link.root.querySelectorAll.mockReturnValue([unrelated, matching])
  const documentLookup = mock(() => { throw new Error("Fragment lookup escaped the Markdown root") })
  setGlobal("document", { querySelector: documentLookup, querySelectorAll: documentLookup, getElementById: documentLookup })

  await openMarkdownLink(link.event, directory)

  expectStopped(link)
  expect(link.root.querySelectorAll.mock.calls).toEqual([["[id]"]])
  expect(matching.scrollIntoView.mock.calls).toEqual([[{ block: "nearest" }]])
  expect(unrelated.scrollIntoView).not.toHaveBeenCalled()
  expect(documentLookup).not.toHaveBeenCalled()
  expect(location.hash).toBe("#/session/current")
  expect(invoke).not.toHaveBeenCalled()
  expect(fetchMock).not.toHaveBeenCalled()
  expect(window.open).not.toHaveBeenCalled()
})

test("companion #/ navigation is left intact", async () => {
  runtime("http://192.168.1.8:41718/companion")
  const link = click("#/workspace/project/session/ses_1")

  await openMarkdownLink(link.event, directory)

  expect(link.preventDefault).not.toHaveBeenCalled()
  expect(link.stopPropagation).not.toHaveBeenCalled()
  expect(link.event.defaultPrevented).toBe(false)
  expect(link.root.querySelectorAll).not.toHaveBeenCalled()
  expect(invoke).not.toHaveBeenCalled()
  expect(fetchMock).not.toHaveBeenCalled()
})

test("already defaultPrevented clicks are ignored before looking up an anchor", async () => {
  native()
  const link = click(rawContract, { defaultPrevented: true })

  await openMarkdownLink(link.event, directory)

  expect(link.target.closest).not.toHaveBeenCalled()
  expect(link.preventDefault).not.toHaveBeenCalled()
  expect(link.stopPropagation).not.toHaveBeenCalled()
  expect(invoke).not.toHaveBeenCalled()
  expect(fetchMock).not.toHaveBeenCalled()
})

test("middle-click opens a local file and cancels auxiliary browser navigation", async () => {
  native()
  const link = click(rawContract, { button: 1 })

  await openMarkdownLink(link.event, directory)

  expect(invoke.mock.calls).toEqual([["open_file_in_editor", { path: contractPath, line: undefined, column: undefined }]])
  expectStopped(link)
})

test("remote /companion sends open_file_in_editor to the host RPC instead of opening a browser URL", async () => {
  const location = runtime("http://192.168.1.8:41718/companion")
  fetchMock.mockImplementation(async () => Response.json({ positioned: true }))
  const link = click(`${rawContract}#L12C3`)

  await openMarkdownLink(link.event, directory)

  expect(window.location).toBe(globalThis.location)
  expect(fetchMock).toHaveBeenCalledTimes(1)
  const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit]
  expect(url).toBe("/api/invoke")
  expect(new URL(url, location.href).href).toBe("http://192.168.1.8:41718/api/invoke")
  expect(init.method).toBe("POST")
  expect(init.credentials).toBe("same-origin")
  expect(init.headers).toEqual({ "content-type": "application/json" })
  expect(JSON.parse(init.body as string)).toEqual({ command: "open_file_in_editor", args: { path: contractPath, line: 12, column: 3 } })
  expect(invoke).not.toHaveBeenCalled()
  expect(window.open).not.toHaveBeenCalled()
  expectStopped(link)
})

test("native file-open failures propagate without falling back to external navigation", async () => {
  native()
  const failure = new Error("The editor could not open this file")
  invoke.mockRejectedValue(failure)
  const link = click(rawContract)

  await expect(openMarkdownLink(link.event, directory)).rejects.toBe(failure)

  expect(invoke.mock.calls).toEqual([["open_file_in_editor", { path: contractPath, line: undefined, column: undefined }]])
  expect(fetchMock).not.toHaveBeenCalled()
  expectStopped(link)
})

test("forged copy-control attributes fall through to link dispatch instead of browser navigation", async () => {
  native()
  const link = click(rawContract)
  const forged = { closest: () => ({}) }
  link.target.closest.mockImplementation((selector) => selector === "[data-copy-code]" ? forged : link.anchor)

  await markdownClick(link.event, directory)

  expect(invoke.mock.calls).toEqual([["open_file_in_editor", { path: contractPath, line: undefined, column: undefined }]])
  expectStopped(link)
})

test("encoded newlines in web URLs still reach the external opener unchanged", async () => {
  native()
  const url = "https://mail.google.com/mail/?view=cm&body=first%0Asecond"
  const link = click(url)
  await openMarkdownLink(link.event, directory)
  expect(invoke.mock.calls).toEqual([["plugin:opener|open_url", { url }]])
  expectStopped(link)
})

test("existing explicit file actions retain OS-associated opening", async () => {
  native()
  const { openFile } = await import("../src/tool-actions")
  await openFile("C:/work/file.pdf")
  expect(invoke.mock.calls).toEqual([["open_file", { path: "C:/work/file.pdf", line: undefined, column: undefined }]])
})

test("an older host rejecting the new command cannot fall back to OS-associated opening", async () => {
  native()
  invoke.mockRejectedValue(new Error("Command open_file_in_editor not found"))
  const link = click("./payload.cmd")
  await expect(openMarkdownLink(link.event, directory)).rejects.toThrow("not found")
  expect(invoke).toHaveBeenCalledTimes(1)
  expect(invoke.mock.calls[0][0]).toBe("open_file_in_editor")
  expectStopped(link)
})

test.each([
  ["./docs/notes.md#design%20notes", "docs/notes.md", undefined, undefined, "design notes"],
  ["./docs/notes.md#L42C7", "docs/notes.md", 42, 7, "L42C7"],
  ["./report.pdf#page=3", "report.pdf", undefined, undefined, "page=3"],
  ["./photo.png", "photo.png", undefined, undefined, undefined],
  ["./photo.JPG", "photo.JPG", undefined, undefined, undefined],
  ["./icon.svg", "icon.svg", undefined, undefined, undefined],
  ["./docs/design%23notes.md", "docs/design#notes.md", undefined, undefined, undefined],
])("All routes %s to the modal without opening an editor", async (raw, relativePath, line, column, hash) => {
  native()
  prefs.setFilePreviewMode("all")
  // All overrides even previously disabled Custom switches.
  for (const type of filePreviewTypes) prefs.setFilePreviewType(type, false)
  const link = click(raw)

  await openMarkdownLink(link.event, directory)

  expect(previewFile()).toMatchObject({
    path: `C:/Users/Kyle/Desktop/C++/Drift/${relativePath}`, directory, hash,
  })
  expect(previewFile()?.line).toBe(line)
  expect(previewFile()?.column).toBe(column)
  expect(invoke).not.toHaveBeenCalled()
  expect(fetchMock).not.toHaveBeenCalled()
  expect(window.open).not.toHaveBeenCalled()
  expectStopped(link)
})

test.each([
  ["markdown", "notes.md"], ["pdf", "report.pdf"], ["image", "photo.png"],
  ["text", "main.ts"], ["table", "data.csv"], ["audio", "sound.mp3"], ["video", "movie.mp4"],
] as [FilePreviewType, string][])("Custom routes selected %s to the modal, disabled to the editor", async (type, filename) => {
  native()
  prefs.setFilePreviewMode("custom")
  for (const candidate of filePreviewTypes) prefs.setFilePreviewType(candidate, candidate === type)
  const enabled = click(`./${filename}#L12C3`)
  const path = `C:/Users/Kyle/Desktop/C++/Drift/${filename}`

  await openMarkdownLink(enabled.event, directory)

  expect(previewFile()).toMatchObject({ path, directory, line: 12, column: 3, hash: "L12C3" })
  expect(invoke).not.toHaveBeenCalled()
  expectStopped(enabled)
  closeFilePreview()
  prefs.setFilePreviewType(type, false)
  const disabled = click(`./${filename}#L12C3`)

  await openMarkdownLink(disabled.event, directory)

  expect(previewFile()).toBeUndefined()
  expect(invoke.mock.calls).toEqual([["open_file_in_editor", { path, line: 12, column: 3 }]])
  expect(fetchMock).not.toHaveBeenCalled()
  expect(window.open).not.toHaveBeenCalled()
  expectStopped(disabled)
})

test.each(["notes.md", "report.pdf", "photo.png", "main.ts"])("None keeps %s in the editor", async (filename) => {
  native()
  const link = click(`./${filename}`)

  await openMarkdownLink(link.event, directory)

  expect(previewFile()).toBeUndefined()
  expect(invoke.mock.calls).toEqual([["open_file_in_editor", {
    path: `C:/Users/Kyle/Desktop/C++/Drift/${filename}`, line: undefined, column: undefined,
  }]])
  expectStopped(link)
})

test.each(["payload.exe", "archive.zip", "report.docx", "unknown"])("All keeps unsupported %s editor-only", async (filename) => {
  native()
  prefs.setFilePreviewMode("all")
  const link = click(`./${filename}`)

  await openMarkdownLink(link.event, directory)

  expect(previewFile()).toBeUndefined()
  expect(invoke.mock.calls).toEqual([["open_file_in_editor", {
    path: `C:/Users/Kyle/Desktop/C++/Drift/${filename}`, line: undefined, column: undefined,
  }]])
  expect(fetchMock).not.toHaveBeenCalled()
  expect(window.open).not.toHaveBeenCalled()
  expectStopped(link)
})

test("remote preview captures the host path without opening anything on the host", async () => {
  runtime("http://192.168.1.8:41718/companion")
  prefs.setFilePreviewMode("all")
  const link = click(`${rawContract}#L12C3`)

  await openMarkdownLink(link.event, directory)

  expect(previewFile()).toMatchObject({ path: contractPath, directory, line: 12, column: 3, hash: "L12C3" })
  expect(invoke).not.toHaveBeenCalled()
  expect(fetchMock).not.toHaveBeenCalled()
  expect(window.open).not.toHaveBeenCalled()
  expectStopped(link)
})

test.each([
  ["./child.md", "C:/workspace/docs/nested/child.md"],
  ["../../README.md", "C:/workspace/README.md"],
  ["../../../outside.md", "C:/outside.md"],
])("nested preview %s retains the original read boundary instead of widening it", async (raw, path) => {
  native()
  prefs.setFilePreviewMode("all")
  const parentDirectory = "C:/workspace/docs/nested"
  const workspaceRoot = "C:/workspace"
  const link = click(`${raw}#L9`)

  await openMarkdownLink(link.event, parentDirectory, workspaceRoot)

  // Even an outside target must retain the root for the backend's boundary check.
  expect(previewFile()).toMatchObject({ path, directory: workspaceRoot, line: 9, hash: "L9" })
  expect(invoke).not.toHaveBeenCalled()
  expect(fetchMock).not.toHaveBeenCalled()
  expectStopped(link)
})

test.each([false, true])("delegated Markdown dispatch preserves the nested root with forged copy control %s", async (forged) => {
  native()
  prefs.setFilePreviewMode("all")
  const link = click("../notes.md")
  link.target.closest.mockImplementation((selector) => selector === "[data-copy-code]"
    ? forged ? { closest: () => ({}) } : null
    : link.anchor)

  await markdownClick(link.event, "C:/workspace/docs/nested", "C:/workspace")

  expect(previewFile()).toMatchObject({ path: "C:/workspace/docs/notes.md", directory: "C:/workspace" })
  expect(invoke).not.toHaveBeenCalled()
  expectStopped(link)
})

test("All middle-clicks use the modal and still cancel auxiliary navigation", async () => {
  native()
  prefs.setFilePreviewMode("all")
  const link = click(rawContract, { button: 1 })
  await openMarkdownLink(link.event, directory)
  expect(previewFile()).toMatchObject({ path: contractPath, directory })
  expect(invoke).not.toHaveBeenCalled()
  expectStopped(link)
})

test("All without a backend still rejects rather than creating an unusable preview", async () => {
  prefs.setFilePreviewMode("all")
  const link = click(rawContract)
  await expect(openMarkdownLink(link.event, directory)).rejects.toThrow("Opening files requires the Drift host backend")
  expect(previewFile()).toBeUndefined()
  expect(fetchMock).not.toHaveBeenCalled()
  expect(window.open).not.toHaveBeenCalled()
  expectStopped(link)
})

test.each(["all", "custom", "none"] as const)("%s does not change external HTTPS dispatch", async (mode) => {
  native()
  prefs.setFilePreviewMode(mode)
  const url = "https://example.com/report.pdf#page=2"
  const link = click(url)
  await openMarkdownLink(link.event, directory)
  expect(invoke.mock.calls).toEqual([["plugin:opener|open_url", { url }]])
  expect(previewFile()).toBeUndefined()
  expectStopped(link)
})

test.each(["https://example.com/notes.md", "mailto:help@example.com", "tel:+15551234567"])("remote All leaves external %s to browser navigation", async (raw) => {
  runtime("http://192.168.1.8:41718/companion")
  prefs.setFilePreviewMode("all")
  const link = click(raw)
  await openMarkdownLink(link.event, directory)
  expect(link.preventDefault).not.toHaveBeenCalled()
  expect(link.stopPropagation).not.toHaveBeenCalled()
  expect(previewFile()).toBeUndefined()
  expect(invoke).not.toHaveBeenCalled()
  expect(fetchMock).not.toHaveBeenCalled()
  expect(window.open).not.toHaveBeenCalled()
})

test("all four Markdown callers pass their owning session directory, including delegated output", async () => {
  const sources = await Promise.all(["message", "parts"].map((name) => Bun.file(new URL(`../src/ui/${name}.tsx`, import.meta.url)).text()))
  const callers = sources.flatMap((source) => [...source.matchAll(/<Markdown\b[\s\S]*?\/>/g)].map(([tag]) => tag))
  expect(callers).toHaveLength(4)
  expect(callers.map((tag) => tag.match(/\bdirectory=\{([^}]+)\}/)?.[1].replace(/\s+/g, ""))).toEqual([
    "engine.state.sessions[info().sessionID]?.directory",
    "engine.state.sessions[part().sessionID]?.directory",
    "engine.state.sessions[props.part.sessionID]?.directory",
    "engine.state.sessions[delegatedChildId(engine.state,props.part)??props.part.sessionID]?.directory",
  ])
})

test("Markdown wires click and middle auxclick through its directory-aware handlers", async () => {
  const source = await Bun.file(new URL("../src/ui/markdown.tsx", import.meta.url)).text()
  expect(source).toMatch(/onClick=\{handleClick\}/)
  expect(source).toMatch(/onAuxClick=\{\(event\)\s*=>\s*\{\s*if\s*\(event\.button\s*===\s*1\)\s*void handleClick\(event\)/)
  expect(source).toMatch(/if\s*\(event\.type\s*===\s*"auxclick"\)\s*await openMarkdownLink\(event,\s*props\.directory,\s*props\.workspaceDirectory\s*\?\?\s*props\.directory\)/)
  expect(source).toMatch(/else\s+await markdownClick\(event,\s*props\.directory,\s*props\.workspaceDirectory\s*\?\?\s*props\.directory\)/)
  expect(source).toMatch(/if\s*\(!button\)\s*return openMarkdownLink\(event,\s*directory,\s*workspaceDirectory\)/)
})
