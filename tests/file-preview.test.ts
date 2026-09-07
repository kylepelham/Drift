import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test"
import { previewParentDirectory, previewTable, readFilePreview } from "../src/file-preview"
import { filePreviewLimits } from "../src/file-preview-types"
import { closeFilePreview, openFilePreview, previewFile } from "../src/state/file-preview"
import { t } from "../src/state/i18n"

const globalNames = ["__TAURI__", "window", "location", "fetch"] as const
let originalGlobals: (PropertyDescriptor | undefined)[]
let invoke: ReturnType<typeof mock>
let fetchMock: ReturnType<typeof mock>
let response: unknown
const request = { path: "C:/workspace/docs/notes.md", directory: "C:/workspace", line: 12, column: 3, hash: "L12C3" }

function setGlobal(name: (typeof globalNames)[number], value: unknown) {
  Object.defineProperty(globalThis, name, { configurable: true, writable: true, value })
}

function encoded(content: string | number[]) {
  const bytes = typeof content === "string" ? new TextEncoder().encode(content) : Uint8Array.from(content)
  return { content: Buffer.from(bytes).toString("base64"), size: bytes.length }
}

beforeEach(() => {
  originalGlobals = globalNames.map((name) => Object.getOwnPropertyDescriptor(globalThis, name))
  closeFilePreview()
  response = encoded("# Preview\n")
  invoke = mock(async (_command: string, _args?: Record<string, unknown>) => response)
  fetchMock = mock(async (_url: string, _init?: RequestInit) => Response.json(response))
  const location = new URL("http://localhost:5180/")
  setGlobal("location", location)
  setGlobal("window", { location, open: mock(() => { throw new Error("Unexpected window.open") }) })
  setGlobal("__TAURI__", undefined)
  setGlobal("fetch", fetchMock)
})

afterEach(() => {
  closeFilePreview()
  globalNames.forEach((name, index) => {
    const descriptor = originalGlobals[index]
    if (descriptor) Object.defineProperty(globalThis, name, descriptor)
    else Reflect.deleteProperty(globalThis, name)
  })
})

test("controller captures a request independently of later caller mutation", () => {
  const input = { ...request }
  openFilePreview(input)
  expect(previewFile()).toEqual(request)
  expect(previewFile()).not.toBe(input)

  input.path = "D:/other/notes.md"
  input.directory = "D:/other"
  input.line = 99
  input.column = 8
  input.hash = "changed"
  expect(previewFile()).toEqual(request)
  expect(invoke).not.toHaveBeenCalled()
  expect(fetchMock).not.toHaveBeenCalled()
})

test("controller replaces the current request, creates a fresh identity on reopen, and closes idempotently", () => {
  expect(previewFile()).toBeUndefined()
  openFilePreview(request)
  const first = previewFile()
  openFilePreview(request)
  expect(previewFile()).toEqual(first)
  expect(previewFile()).not.toBe(first)

  const next = { path: "C:/workspace/report.pdf", directory: request.directory }
  openFilePreview(next)
  expect(previewFile()).toEqual(next)
  expect(previewFile()?.line).toBeUndefined()
  expect(previewFile()?.hash).toBeUndefined()
  closeFilePreview()
  closeFilePreview()
  expect(previewFile()).toBeUndefined()
  openFilePreview(next)
  expect(previewFile()).toEqual(next)
})

for (const route of ["native", "remote"] as const) {
  describe(`${route} bounded reader through the real backendInvoke`, () => {
    beforeEach(() => {
      if (route === "native") setGlobal("__TAURI__", { core: { invoke } })
      else {
        const location = new URL("http://192.168.1.8:41718/companion")
        setGlobal("location", location)
        setGlobal("window", { location, open: mock(() => { throw new Error("Unexpected window.open") }) })
      }
    })

    afterEach(() => {
      if (route === "native") expect(fetchMock).not.toHaveBeenCalled()
      else expect(invoke).not.toHaveBeenCalled()
    })

    test("sends the captured path, workspace boundary, and byte limit only", async () => {
      const result = await readFilePreview(request)
      expect(result).toEqual({ kind: "markdown", bytes: new TextEncoder().encode("# Preview\n"), text: "# Preview\n" })
      const args = { path: request.path, directory: request.directory, maxBytes: filePreviewLimits.markdown }
      if (route === "native") expect(invoke.mock.calls).toEqual([["read_file_preview", args]])
      else {
        expect(fetchMock).toHaveBeenCalledTimes(1)
        const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit]
        expect(url).toBe("/api/invoke")
        expect(init.method).toBe("POST")
        expect(init.credentials).toBe("same-origin")
        expect(init.headers).toEqual({ "content-type": "application/json" })
        expect(JSON.parse(init.body as string)).toEqual({ command: "read_file_preview", args })
      }
      expect(window.open).not.toHaveBeenCalled()
    })

    test.each([
      ["notes.md", "markdown"], ["main.ts", "text"], ["plan.html", "text"], ["plan.HTM", "text"], ["data.csv", "table"], ["data.tsv", "table"],
    ] as const)("%s preserves text whitespace and counts UTF-8 bytes rather than characters", async (filename, kind) => {
      const text = " \t\r\n  caf\u00e9 \u{1f642}\n\t "
      response = encoded(text)
      const result = await readFilePreview({ ...request, path: `C:/workspace/${filename}` })
      expect(result.kind).toBe(kind)
      expect(result.text).toBe(text)
      expect(result.bytes).toEqual(new TextEncoder().encode(text))
      expect(result.bytes.length).toBeGreaterThan(text.length)
      const args = route === "native" ? invoke.mock.calls[0][1] : JSON.parse(fetchMock.mock.calls[0][1].body).args
      expect(args.maxBytes).toBe(filePreviewLimits[kind])
    })

    test("base64 transport whitespace is decoded without changing the byte count", async () => {
      response = { content: " \tY W\r\nJj\n", size: 3 }
      expect((await readFilePreview(request)).text).toBe("abc")
    })

    test("an empty text file is valid", async () => {
      response = { content: "", size: 0 }
      expect(await readFilePreview(request)).toEqual({ kind: "markdown", bytes: new Uint8Array(), text: "" })
    })

    test.each([
      ["report.pdf", "pdf"], ["photo.png", "image"], ["sound.mp3", "audio"], ["movie.mp4", "video"],
    ] as const)("%s stays binary and uses its own size limit", async (filename, kind) => {
      response = encoded([0, 255, 128, 195, 40])
      const path = `C:/workspace/${filename}`
      const result = await readFilePreview({ ...request, path })
      expect(result).toEqual({ kind, bytes: Uint8Array.from([0, 255, 128, 195, 40]), text: undefined })
      const args = { path, directory: request.directory, maxBytes: filePreviewLimits[kind] }
      if (route === "native") expect(invoke.mock.calls).toEqual([["read_file_preview", args]])
      else expect(JSON.parse(fetchMock.mock.calls[0][1].body)).toEqual({ command: "read_file_preview", args })
    })

    test.each(["notes.md", "main.ts", "plan.html", "data.csv"])("%s rejects NUL-containing binary disguised as text", async (filename) => {
      response = encoded("plain\0binary")
      await expect(readFilePreview({ ...request, path: `C:/workspace/${filename}` })).rejects.toThrow(t("drift.preview.unsupported"))
    })

    test.each([
      ["notes.md", [0xc3, 0x28]],
      ["main.ts", [0xff]],
      ["plan.htm", [0xff]],
      ["data.csv", [0xe2, 0x82]],
      ["data.tsv", [0xc0, 0xaf]],
    ] as [string, number[]][])("%s rejects malformed UTF-8 instead of replacing bytes", async (filename, bytes) => {
      response = encoded(bytes)
      await expect(readFilePreview({ ...request, path: `C:/workspace/${filename}` })).rejects.toThrow()
    })

    test.each(["%%%", "a", "Y=Jj", "YWJj===="])("rejects invalid base64 %s", async (content) => {
      response = { content, size: 3 }
      await expect(readFilePreview(request)).rejects.toThrow()
    })

    test.each([
      null, {}, { content: "" }, { size: 0 }, { content: 0, size: 0 },
      { content: "", size: "0" }, { content: "", size: " " }, { content: "", size: -1 }, { content: "", size: 0.5 },
      { content: "", size: NaN }, { content: "", size: Infinity },
      { content: "", size: Number.MAX_SAFE_INTEGER + 1 },
    ])("rejects malformed backend response %j", async (value) => {
      response = value
      await expect(readFilePreview(request)).rejects.toThrow(t("drift.preview.error"))
    })

    test.each([0, 2, 4])("rejects a declared size of %i for three decoded bytes", async (size) => {
      response = { content: "YWJj", size }
      await expect(readFilePreview(request)).rejects.toThrow(t("drift.preview.error"))
    })

    test("accepts text exactly at the byte limit", async () => {
      const text = "a".repeat(filePreviewLimits.markdown)
      response = encoded(text)
      const result = await readFilePreview(request)
      expect(result.bytes.length).toBe(filePreviewLimits.markdown)
      expect(result.text).toBe(text)
    })

    test("rejects a declared size above the limit before attempting base64 decoding", async () => {
      response = { content: "%", size: filePreviewLimits.markdown + 1 }
      await expect(readFilePreview(request)).rejects.toThrow(t("drift.preview.tooLarge"))
    })

    test("bounds encoded content length even when the declared size is small", async () => {
      response = { content: "%".repeat(Math.ceil(filePreviewLimits.markdown / 3) * 4 + 1), size: 1 }
      await expect(readFilePreview(request)).rejects.toThrow(t("drift.preview.tooLarge"))
    })

    test("rejects decoded content above the limit even when base64 rounding hides the extra byte", async () => {
      response = { ...encoded("a".repeat(filePreviewLimits.markdown + 1)), size: filePreviewLimits.markdown }
      await expect(readFilePreview(request)).rejects.toThrow(t("drift.preview.error"))
    })

    test("does not let base64 whitespace bypass the encoded content bound", async () => {
      response = { content: " ".repeat(Math.ceil(filePreviewLimits.markdown / 3) * 4 + 1), size: 0 }
      await expect(readFilePreview(request)).rejects.toThrow(t("drift.preview.tooLarge"))
    })

    test("propagates backend read failures without attempting another opener", async () => {
      const message = "File is outside the workspace"
      invoke.mockRejectedValue(new Error(message))
      fetchMock.mockImplementation(async () => Response.json({ error: message }, { status: 403 }))
      await expect(readFilePreview(request)).rejects.toThrow(message)
      expect(route === "native" ? invoke : fetchMock).toHaveBeenCalledTimes(1)
      expect(window.open).not.toHaveBeenCalled()
    })

    test("rejects unsupported files without making a backend request", async () => {
      await expect(readFilePreview({ ...request, path: "C:/workspace/archive.zip" })).rejects.toThrow(t("drift.preview.unsupported"))
      expect(invoke).not.toHaveBeenCalled()
      expect(fetchMock).not.toHaveBeenCalled()
    })
  })
}

test("a plain browser without a backend cannot read a preview", async () => {
  await expect(readFilePreview(request)).rejects.toThrow(t("drift.preview.unavailable"))
  expect(invoke).not.toHaveBeenCalled()
  expect(fetchMock).not.toHaveBeenCalled()
  expect(window.open).not.toHaveBeenCalled()
})

test.each([
  ["C:\\workspace\\docs\\notes.md", "C:/workspace/docs"],
  ["C:/notes.md", "C:/"], ["/notes.md", "/"], ["/workspace/docs/notes.md", "/workspace/docs"],
  ["\\\\server\\share\\notes.md", "//server/share"], ["notes.md", "/"],
])("parent directory of %s preserves path roots", (path, expected) => {
  expect(previewParentDirectory(path)).toBe(expected)
})

test("CSV handles quoted delimiters, multiline CRLF, escaped quotes, and empty cells", () => {
  const text = 'name,description,empty\r\n"Doe, Jane","first\r\nsecond ""quoted""",\r\nplain,last,\r\n'
  expect(previewTable(text, ",")).toEqual({
    rows: [["name", "description", "empty"], ["Doe, Jane", 'first\r\nsecond "quoted"', ""], ["plain", "last", ""]],
    truncated: false,
  })
})

test("TSV keeps quoted tabs and embedded newlines within a cell", () => {
  expect(previewTable('name\tvalue\n"two\twords"\t"one\ntwo"\n', "\t")).toEqual({
    rows: [["name", "value"], ["two\twords", "one\ntwo"]], truncated: false,
  })
})

test.each(["\n", "\r\n", "\r"])("table recognizes record separator %j without adding a trailing row", (separator) => {
  expect(previewTable(`a,b${separator}c,d${separator}`, ",")).toEqual({ rows: [["a", "b"], ["c", "d"]], truncated: false })
})

test("table preserves spaces, blank rows, trailing empty columns, and a final unterminated record", () => {
  expect(previewTable(" a , b \n\nlast,", ",")).toEqual({ rows: [[" a ", " b "], [""], ["last", ""]], truncated: false })
  expect(previewTable("", ",")).toEqual({ rows: [], truncated: false })
})

test("table returns escaped quotes and HTML-looking payloads as literal cell text", () => {
  expect(previewTable('"<img src=x onerror=""alert(1)"">","&lt;script&gt;"', ",")).toEqual({
    rows: [['<img src=x onerror="alert(1)">', "&lt;script&gt;"]], truncated: false,
  })
})

test("table row limits count records, not quoted newlines, and report remaining content", () => {
  expect(previewTable('"one\ntwo",a\r\nthree,b\r\nfour,c', ",", 2)).toEqual({
    rows: [["one\ntwo", "a"], ["three", "b"]], truncated: true,
  })
  for (const ending of ["", "\n", "\r\n"]) {
    expect(previewTable(`one,a\ntwo,b${ending}`, ",", 2)).toEqual({ rows: [["one", "a"], ["two", "b"]], truncated: false })
  }
})

test("table column limits discard extra cells without shifting the next record", () => {
  expect(previewTable('a,b,"hidden\nfield",d\r\ne,f,g', ",", 10, 2)).toEqual({
    rows: [["a", "b"], ["e", "f"]], truncated: true,
  })
  expect(previewTable("a,b\nc,d", ",", 10, 2)).toEqual({ rows: [["a", "b"], ["c", "d"]], truncated: false })
})

test("default table render window is bounded to 200 rows and 50 columns", () => {
  const exact = Array.from({ length: 200 }, (_, row) => Array.from({ length: 50 }, (_, column) => `${row}:${column}`).join(",")).join("\n")
  const full = previewTable(exact, ",")
  expect(full.rows).toHaveLength(200)
  expect(full.rows.every((row) => row.length === 50)).toBe(true)
  expect(full.rows[199][49]).toBe("199:49")
  expect(full.truncated).toBe(false)
  expect(previewTable(`${exact}\nextra,row`, ",")).toEqual({ ...full, truncated: true })

  const wide = previewTable(Array.from({ length: 201 }, () => Array.from({ length: 51 }, (_, index) => `${index}`).join(",")).join("\n"), ",")
  expect(wide.rows).toHaveLength(200)
  expect(wide.rows.every((row) => row.length === 50 && row[49] === "49")).toBe(true)
  expect(wide.truncated).toBe(true)
})

test("preview host source uses keyed modal cleanup and ignores late read results", async () => {
  const source = await Bun.file(new URL("../src/ui/file-preview.tsx", import.meta.url)).text()
  expect(source).toMatch(/<Show when=\{previewFile\(\)\} keyed>/)
  expect(source).toContain("<Portal><FilePreviewDialog file={file} /></Portal>")
  expect(source).toContain("activateModal(dialog, closeFilePreview, { nativeTabOrder: true })")
  expect(source).toContain("closeOnBackdropPointerDown(event, closeFilePreview, dialog)")
  expect(source).toContain("onClick={closeFilePreview}")
  expect(source).toMatch(/onCleanup\(\(\) => \{\s*disposed = true\s*if \(objectUrl\) URL\.revokeObjectURL\(objectUrl\)/)
  expect(source).toMatch(/readFilePreview\(props\.file\)\.then\(\(result\) => \{\s*if \(disposed\) return/)
  expect(source).toContain("if (!disposed) setError(")
  expect(source).toContain("onClick={() => setAttempt((value) => value + 1)}")
  expect(source).toContain("editorOnly: true")
})

test("preview renderers source preserves the workspace root and renders bounded table cells as text", async () => {
  const source = await Bun.file(new URL("../src/ui/file-preview.tsx", import.meta.url)).text()
  expect(source).toContain("<MarkdownDocument text={file().text!} path={props.file.path} directory={props.file.directory}")
  expect(source).toContain('previewTable(props.text, /\\.tsv$/i.test(props.path) ? "\\t" : ",")')
  expect(source).toContain("<For each={table().rows}>")
  expect(source).toMatch(/<For each=\{row\}>\{\(cell\) => <td\b[^>]*>\{cell\}<\/td>/)
  expect(source).not.toContain("innerHTML")
  expect(source).not.toMatch(/<(?:iframe|object|embed)\b/)
})
