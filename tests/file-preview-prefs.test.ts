import { expect, test } from "bun:test"
import { filePreviewLimits, filePreviewMime, filePreviewType, filePreviewTypes, type FilePreviewType } from "../src/file-preview-types"
import {
  filePreviewAllowed,
  filePreviewPrefs,
  normalizeFilePreviewPrefs,
  setFilePreviewMode,
  setFilePreviewType,
  shouldPreviewFile,
} from "../src/state/file-preview-prefs"
import { persisted } from "../src/state/persist"

const samples: Record<FilePreviewType, string[]> = {
  markdown: ["README.md", "notes.markdown", "notes.mdown"],
  pdf: ["report.pdf"],
  image: ["photo.png", "photo.jpg", "photo.jpeg", "photo.jfif", "motion.gif", "photo.webp", "photo.avif", "photo.bmp", "icon.ico", "icon.svg", "motion.apng"],
  text: ["notes.txt", "app.log", "index.html", "index.htm", "index.xhtml", "app.ts", "app.tsx", "app.jsx", "app.mjs", "app.cjs", "app.vue", "app.svelte", "app.astro", "app.py", "app.rs", "app.go", "app.cpp", "app.h", "app.java", "app.cs", "app.rb", "app.php", "app.swift", "app.sh", "app.ps1", "app.bat", "app.sql", "config.json", "config.jsonc", "events.jsonl", "config.yaml", "config.yml", "config.toml", "config.ini", "document.xml", "style.css", "style.scss", "changes.diff", "changes.patch", "yarn.lock", "CMakeLists.txt"],
  table: ["data.csv", "data.tsv"],
  audio: ["sound.mp3", "sound.wav", "sound.ogg", "sound.oga", "sound.m4a", "sound.aac", "sound.flac", "sound.opus", "sound.aif", "sound.aiff"],
  video: ["movie.mp4", "movie.m4v", "movie.webm", "movie.ogv", "movie.mov"],
}

test("preview catalogue exports the shared renderer order and byte limits", () => {
  expect(filePreviewTypes).toEqual(["markdown", "pdf", "image", "text", "table", "audio", "video"])
  expect(filePreviewLimits).toEqual({
    markdown: 2_097_152, pdf: 20_971_520, image: 10_485_760, text: 2_097_152,
    table: 5_242_880, audio: 20_971_520, video: 41_943_040,
  })
})

test("supported extensions are case insensitive on Windows and POSIX paths", () => {
  for (const type of filePreviewTypes) {
    for (const name of samples[type]) {
      for (const path of [name, `/workspace/${name}`, `C:\\workspace\\${name.toUpperCase()}`]) {
        expect(filePreviewType(path), path).toBe(type)
        expect(filePreviewMime(path), path).not.toBe("application/octet-stream")
      }
    }
  }
})

test("common extensionless names and dotfiles classify as text, not arbitrary unknown files", () => {
  for (const name of ["README", "LICENSE", "Makefile", "Dockerfile", "Containerfile", "Gemfile", "Justfile", ".env", ".env.local", ".env.production.local", ".env.example", ".gitignore", ".gitconfig", ".dockerignore", ".editorconfig", ".npmrc", ".bashrc", ".zshrc", ".prettierrc", ".eslintrc.json"]) {
    expect(filePreviewType(`/workspace/${name.toUpperCase()}`), name).toBe("text")
    expect(filePreviewMime(name)).toBe("text/plain")
  }
  for (const name of ["unknown", ".unknown", ".env.exe", "Dockerfile.exe", "README.zip"]) {
    expect(filePreviewType(name), name).toBeUndefined()
  }
})

test("binary executables, archives, Office documents, and formats without renderers stay unsupported", () => {
  for (const extension of ["exe", "com", "dll", "so", "dylib", "o", "a", "class", "jar", "wasm", "bin", "dat", "db", "sqlite", "zip", "gz", "tar", "tgz", "7z", "rar", "bz2", "xz", "iso", "msi", "dmg", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "rtf", "epub", "psd", "ai", "heic", "tiff", "swf", "blend", "fbx", "glb", "constructor", "__proto__", "toString"]) {
    const path = `report.pdf.${extension.toUpperCase()}`
    expect(filePreviewType(path), path).toBeUndefined()
    expect(filePreviewMime(path), path).toBe("application/octet-stream")
    expect(filePreviewAllowed(path, normalizeFilePreviewPrefs(undefined))).toBeFalse()
  }
})

test("classification uses only the final filename and does not authorize or resolve traversal", () => {
  for (const path of ["../outside.md", "../../outside.md", "C:\\workspace\\..\\outside.MD", "/outside.md", "folder.pdf/../outside.md"]) {
    expect(filePreviewType(path)).toBe("markdown")
    expect(filePreviewAllowed(path, normalizeFilePreviewPrefs(undefined))).toBeTrue()
  }
  for (const path of ["", ".", "..", "folder.pdf/unknown", "folder.md/", "folder.md\\", "file.pdf.exe", "file%2epdf", "file.pdf?download=1", "file.pdf#page=1"]) {
    expect(filePreviewType(path), path).toBeUndefined()
  }
  expect(filePreviewType("file.exe.md")).toBe("markdown")
})

test("MIME types keep HTML and code inert and SVG in the image-only category", () => {
  for (const path of ["page.html", "page.htm", "page.xhtml", "code.js", "code.ts", "data.xml", "data.json"]) {
    expect(filePreviewType(path)).toBe("text")
    expect(filePreviewMime(path)).toBe("text/plain")
  }
  expect(filePreviewType("icon.SVG")).toBe("image")
  expect(filePreviewMime("icon.SVG")).toBe("image/svg+xml")
  expect(filePreviewMime("notes.MDOWN")).toBe("text/markdown")
  expect(filePreviewMime("report.pdf")).toBe("application/pdf")
  expect(filePreviewMime("photo.jpg")).toBe("image/jpeg")
  expect(filePreviewMime("data.csv")).toBe("text/csv")
  expect(filePreviewMime("data.tsv")).toBe("text/tab-separated-values")
  expect(filePreviewMime("sound.m4a")).toBe("audio/mp4")
  expect(filePreviewMime("sound.opus")).toBe("audio/ogg")
  expect(filePreviewMime("movie.webm")).toBe("video/webm")
})

test("normalization defaults to all and repairs corrupt structures without sharing defaults", () => {
  const defaults = { mode: "all", types: Object.fromEntries(filePreviewTypes.map((type) => [type, true])) }
  for (const value of [undefined, null, false, 0, "custom", [], {}, { mode: "CUSTOM" }, { mode: "unknown", types: [] }, { types: "none" }]) {
    expect(normalizeFilePreviewPrefs(value)).toEqual(defaults)
  }
  const first = normalizeFilePreviewPrefs(null)
  first.types.pdf = false
  expect(normalizeFilePreviewPrefs(null)).toEqual(defaults)
})

test("normalization preserves only known boolean switches including explicit false", () => {
  const input = {
    mode: "custom",
    types: { markdown: false, pdf: true, image: "false", text: 0, table: null, audio: false, video: {}, unknown: false, PDF: false },
    unknown: "discard",
  }
  const normalized = normalizeFilePreviewPrefs(input)
  expect(normalized).toEqual({
    mode: "custom",
    types: { markdown: false, pdf: true, image: true, text: true, table: true, audio: false, video: true },
  })
  expect(normalizeFilePreviewPrefs(normalized)).toEqual(normalized)
  expect(input.types.unknown).toBeFalse()
  expect(normalized.types).not.toBe(input.types)
  expect(normalizeFilePreviewPrefs({ mode: "bad", types: { pdf: false } }).types.pdf).toBeFalse()
  expect(normalizeFilePreviewPrefs({ types: Object.create({ pdf: false }) }).types.pdf).toBeTrue()
})

test("pure preview policy applies all, none, and each custom type independently", () => {
  for (const type of filePreviewTypes) {
    const prefs = normalizeFilePreviewPrefs({ mode: "custom", types: { [type]: false } })
    for (const candidate of filePreviewTypes) {
      const path = samples[candidate][0]
      expect(filePreviewAllowed(path, prefs)).toBe(candidate !== type)
      expect(filePreviewAllowed(path, { ...prefs, mode: "all" })).toBeTrue()
      expect(filePreviewAllowed(path, { ...prefs, mode: "none" })).toBeFalse()
    }
    expect(filePreviewAllowed("unknown.zip", prefs)).toBeFalse()
  }
})

test("persisted normalization handles loaded partial preferences and malformed JSON", () => {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "localStorage")
  if (!descriptor) Object.defineProperty(globalThis, "localStorage", { configurable: true, value: { getItem: () => null, setItem: () => {} } })
  const getItem = localStorage.getItem
  try {
    for (const raw of ['{"mode":"custom","types":{"pdf":false,"unknown":true}}', '{"mode":"none","types":{"text":false}}', "null", "[]", "broken json"]) {
      localStorage.getItem = () => raw
      const [loaded] = persisted("drift.preview.test", normalizeFilePreviewPrefs(undefined), normalizeFilePreviewPrefs)
      const expected = raw === "broken json" ? undefined : JSON.parse(raw)
      expect(loaded()).toEqual(normalizeFilePreviewPrefs(expected))
    }
  } finally {
    localStorage.getItem = getItem
    if (!descriptor) Reflect.deleteProperty(globalThis, "localStorage")
  }
})

test("public preference setters persist switches across mode changes", () => {
  const original = normalizeFilePreviewPrefs(filePreviewPrefs())
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "localStorage")
  if (!descriptor) Object.defineProperty(globalThis, "localStorage", { configurable: true, value: { getItem: () => null, setItem: () => {} } })
  const setItem = localStorage.setItem
  const writes = new Map<string, string>()
  localStorage.setItem = (key, value) => { writes.set(key, value) }
  try {
    expect(filePreviewPrefs().mode).toBe("all")
    setFilePreviewType("pdf", false)
    expect(shouldPreviewFile("report.pdf")).toBeTrue()
    setFilePreviewMode("custom")
    expect(shouldPreviewFile("report.pdf")).toBeFalse()
    expect(shouldPreviewFile("README.md")).toBeTrue()
    setFilePreviewMode("none")
    expect(shouldPreviewFile("README.md")).toBeFalse()
    setFilePreviewMode("all")
    expect(shouldPreviewFile("report.pdf")).toBeTrue()
    expect(shouldPreviewFile("archive.zip")).toBeFalse()
    setFilePreviewMode("custom")
    expect(filePreviewPrefs().types.pdf).toBeFalse()
    expect(JSON.parse(writes.get("drift.preview.prefs")!)).toEqual(filePreviewPrefs())
    const before = filePreviewPrefs()
    setFilePreviewMode("invalid" as never)
    setFilePreviewType("unknown" as never, false)
    setFilePreviewType("pdf", "false" as never)
    expect(filePreviewPrefs()).toBe(before)
    setFilePreviewType("pdf", true)
    expect(shouldPreviewFile("report.pdf")).toBeTrue()
  } finally {
    setFilePreviewMode(original.mode)
    for (const type of filePreviewTypes) setFilePreviewType(type, original.types[type])
    localStorage.setItem = setItem
    if (!descriptor) Reflect.deleteProperty(globalThis, "localStorage")
  }
})

test("all 18 locales own preview keys with matching interpolation placeholders", async () => {
  const keys = [
    "settings.title", "settings.mode", "settings.description", "mode.all", "mode.none", "mode.custom",
    ...filePreviewTypes.map((type) => `type.${type}`),
    "title", "loading", "error", "openEditor", "retry", "previous", "next", "page", "zoomIn", "zoomOut",
    "unsupported", "tooLarge", "unavailable", "tableTruncated", "mediaError", "pdfPassword",
  ].map((key) => `drift.preview.${key}`).sort()
  for (const locale of ["en", "ar", "br", "bs", "da", "de", "es", "fr", "ja", "ko", "no", "pl", "ru", "th", "tr", "uk", "zh", "zht"]) {
    const { dict, drift } = await import(`../src/i18n/${locale}.ts`)
    expect(Object.keys(drift).filter((key) => key.startsWith("drift.preview.")).sort(), locale).toEqual(keys)
    for (const key of keys) {
      expect(drift[key].trim().length, `${locale}: ${key}`).toBeGreaterThan(0)
      const placeholders = drift[key].match(/{{\w+}}/g)?.sort() ?? []
      expect(placeholders, `${locale}: ${key}`).toEqual(
        key.endsWith(".page") ? ["{{page}}", "{{pages}}"].sort() : key.endsWith(".tableTruncated") ? ["{{rows}}"] : [],
      )
    }
    expect(dict["common.close"]).toBeTruthy()
  }
  const { drift } = await import("../src/i18n/en")
  expect(drift["drift.preview.page"]).toBe("Page {{page}} of {{pages}}")
  expect(drift["drift.preview.tableTruncated"]).toBe("Showing the first {{rows}} rows")
  expect(drift["drift.preview.pdfPassword"]).toBe("Password-protected PDFs cannot be previewed.")
})
