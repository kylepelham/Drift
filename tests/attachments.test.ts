import { expect, test } from "bun:test"
import {
  attachmentSizeLimits,
  collectPdfText,
  decodeUtf8,
  formatPdfAttachment,
  maxPdfExtractionChars,
  parseCsvAttachment,
  prepareAttachment,
  prepareAttachmentsForSend,
  resolveAttachmentKind,
  unsupportedModelAttachment,
  type StagedAttachment,
} from "../src/attachments"

test("attachment resolver combines MIME, extension, and signatures", () => {
  expect(resolveAttachmentKind({ filename: "photo.png", mime: "application/octet-stream" }).kind).toBe("image")
  expect(resolveAttachmentKind({ filename: "README", mime: "" }).kind).toBe("text")
  expect(resolveAttachmentKind({ filename: "data.bin", mime: "text/csv" }).kind).toBe("unsupported")
  expect(resolveAttachmentKind({ filename: "report", mime: "application/pdf" }).kind).toBe("pdf")
  expect(resolveAttachmentKind({ filename: "values.tsv", mime: "text/plain" }).kind).toBe("csv")
  expect(resolveAttachmentKind({ filename: "clip.unknown", mime: "video/webm" }).kind).toBe("video")
  expect(
    resolveAttachmentKind({ filename: "renamed.txt", mime: "text/plain", bytes: new Uint8Array([0x50, 0x4b, 0x03, 0x04]) }),
  ).toMatchObject({ kind: "unsupported", reason: "archive" })
  expect(
    resolveAttachmentKind({ filename: "notes.txt", mime: "text/plain", bytes: new Uint8Array([0x25, 0x50, 0x44, 0x46]) }),
  ).toMatchObject({ kind: "pdf", mime: "application/pdf" })
})

test("text attachments are strict UTF-8 and become bounded readable prompt text", async () => {
  expect(() => decodeUtf8(new Uint8Array([0xc3, 0x28]))).toThrow()
  const invalid = await prepareAttachment(new File([new Uint8Array([0xc3, 0x28])], "bad.txt", { type: "text/plain" }), "bad")
  expect(invalid).toMatchObject({ ok: false, reason: "invalid-utf8", kind: "text" })

  const prepared = await prepareAttachment(new File(["first\nsecond\nthird"], "sample.ts", { type: "" }), "text")
  expect(prepared.ok).toBeTrue()
  if (!prepared.ok) return
  expect(prepared.attachment.dataUrl).toBeUndefined()
  expect(prepared.attachment.meta).toMatchObject({ lines: 3, language: "typescript" })
  expect(prepared.attachment.text).toContain("[Attachment: sample.ts (typescript)]")
  expect(prepared.attachment.text).toContain("first\nsecond\nthird")
})

test("CSV delimiter, dimensions, quoting, and truncation are deterministic", () => {
  const parsed = parseCsvAttachment('name;note;score\nAda;"hello; world";10\nLin;ok;9')
  expect(parsed.delimiter).toBe(";")
  expect(parsed.rows).toBe(3)
  expect(parsed.columns).toBe(3)
  expect(parsed.preview).toContain("hello; world")
  expect(parsed.truncated).toBeFalse()

  const truncated = parseCsvAttachment("a,b\n" + Array.from({ length: 40 }, (_, index) => `${index},value`).join("\n"), 5)
  expect(truncated.rows).toBe(41)
  expect(truncated.columns).toBe(2)
  expect(truncated.truncated).toBeTrue()
  expect(truncated.preview).toContain("[Table preview truncated]")
})

test("PDF extracted text is page-labeled, bounded, and reports truncation", () => {
  const collected = collectPdfText(["first page", "x".repeat(maxPdfExtractionChars)], 8)
  expect(collected.text).toContain("--- Page 1 ---\nfirst page")
  expect(collected.text.length).toBeLessThanOrEqual(maxPdfExtractionChars)
  expect(collected.extractedPages).toBe(2)
  expect(collected.truncated).toBeTrue()
  const formatted = formatPdfAttachment("guide.pdf", 8, collected)
  expect(formatted).toContain("[Attachment: guide.pdf (PDF, 8 pages; extracted 2 pages; content truncated)]")
})

test("archives, binaries, invalid text, and per-kind size limits reject before staging", async () => {
  expect(resolveAttachmentKind({ filename: "source.zip", mime: "application/zip" })).toMatchObject({
    kind: "unsupported",
    reason: "archive",
  })
  expect(resolveAttachmentKind({ filename: "setup.exe", mime: "application/octet-stream" })).toMatchObject({
    kind: "unsupported",
    reason: "binary",
  })
  const oversized = new File([new Uint8Array(attachmentSizeLimits.text + 1)], "large.txt", { type: "text/plain" })
  expect(await prepareAttachment(oversized, "large")).toMatchObject({
    ok: false,
    reason: "too-large",
    kind: "text",
    limit: attachmentSizeLimits.text,
  })
  expect(new Set(Object.values(attachmentSizeLimits)).size).toBeGreaterThan(1)
})

test("audio and video admission uses advertised model input capabilities", () => {
  const audio = { filename: "speech.mp3", mime: "audio/mpeg" }
  const video = { filename: "demo.mp4", mime: "video/mp4" }
  const model = { capabilities: { input: { audio: true, video: false } } }
  expect(unsupportedModelAttachment([audio], model)).toBeUndefined()
  expect(unsupportedModelAttachment([video], model)).toMatchObject({ kind: "video", attachment: video })
  expect(unsupportedModelAttachment([audio], { capabilities: { input: {} } })).toMatchObject({ kind: "audio" })
})

test("send transformation keeps images native and sends non-images as readable content", async () => {
  const attachment = (value: Partial<StagedAttachment> & Pick<StagedAttachment, "filename" | "mime">): StagedAttachment => ({
    id: value.filename,
    size: 10,
    status: "ready",
    meta: {},
    ...value,
  })
  const result = await prepareAttachmentsForSend([
    attachment({ filename: "screen.png", mime: "image/png", dataUrl: "data:image/png;base64,abc" }),
    attachment({ filename: "notes.md", mime: "text/markdown", text: "[Attachment: notes.md]\nreadable" }),
    attachment({ filename: "data.csv", mime: "text/csv", text: "[Attachment: data.csv]\n| a |" }),
    attachment({ filename: "guide.pdf", mime: "application/pdf", text: "[Attachment: guide.pdf]\nPDF text" }),
    attachment({ filename: "legacy.txt", mime: "text/plain", dataUrl: "data:text/plain;base64,bGVnYWN5" }),
  ])
  expect(result.files).toEqual([{ filename: "screen.png", mime: "image/png", url: "data:image/png;base64,abc" }])
  expect(result.text).toContain("readable")
  expect(result.text).toContain("| a |")
  expect(result.text).toContain("PDF text")
  expect(result.text).toContain("legacy")
  expect(result.text).not.toContain("data:image")
})
