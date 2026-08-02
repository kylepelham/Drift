import { getDocument, GlobalWorkerOptions } from "pdfjs-dist"
import workerSrc from "pdfjs-dist/build/pdf.worker.mjs?url"
import { collectPdfText, formatPdfAttachment, maxPdfExtractionChars, maxPdfExtractionPages } from "./attachments"

GlobalWorkerOptions.workerSrc = workerSrc

export async function extractPdfAttachment(data: ArrayBuffer, filename: string) {
  const loading = getDocument({ data: new Uint8Array(data) })
  const pdf = await loading.promise
  try {
    const count = Math.min(pdf.numPages, maxPdfExtractionPages)
    const pages: string[] = []
    let extractedChars = 0
    let thumbnail: string | undefined
    for (let number = 1; number <= count; number++) {
      const page = await pdf.getPage(number)
      const content = await page.getTextContent()
      const text = content.items
          .flatMap((item) => ("str" in item ? [item.str + (item.hasEOL ? "\n" : " ")] : []))
          .join("")
          .replace(/[ \t]+\n/g, "\n")
          .trim()
      pages.push(text)
      extractedChars += text.length
      if (number === 1 && typeof document !== "undefined") thumbnail = await renderThumbnail(page)
      if (extractedChars >= maxPdfExtractionChars) break
    }
    const collected = collectPdfText(pages, pdf.numPages)
    return {
      pages: pdf.numPages,
      extractedPages: collected.extractedPages,
      truncated: collected.truncated,
      thumbnail,
      text: formatPdfAttachment(filename, pdf.numPages, collected),
    }
  } finally {
    await loading.destroy()
  }
}

async function renderThumbnail(page: Awaited<ReturnType<Awaited<ReturnType<typeof getDocument>["promise"]>["getPage"]>>) {
  const base = page.getViewport({ scale: 1 })
  const scale = Math.min(1.5, 280 / base.width)
  const viewport = page.getViewport({ scale })
  const outputScale = window.devicePixelRatio || 1
  const canvas = document.createElement("canvas")
  const context = canvas.getContext("2d")
  if (!context) return
  canvas.width = Math.floor(viewport.width * outputScale)
  canvas.height = Math.floor(viewport.height * outputScale)
  await page.render({
    canvas,
    canvasContext: context,
    viewport,
    transform: outputScale === 1 ? undefined : [outputScale, 0, 0, outputScale, 0, 0],
  }).promise
  return canvas.toDataURL("image/png")
}
