import { createHash } from "node:crypto"
import { closeSync, fstatSync, openSync, readSync, realpathSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"

export type Range = { start: number; end: number }
const chunkSize = 1024 * 1024
const overlap = 4096
const markerLimit = 8
const tempRoot = path.join(tmpdir(), "opencode")
const identifierPattern = /\b(?:CLAUDE_CODE_|ANTHROPIC_|BUN_|DISABLE_|ENABLE_)[A-Z][A-Z0-9_]{2,80}\b/g
const prefixes = ["CLAUDE_CODE_", "ANTHROPIC_", "BUN_", "DISABLE_", "ENABLE_"]

export const defaultMarkers = [
  "Bun v", "bun:main", "BUN_BE_BUN", "__bun", "sourceMappingURL=", "source-map",
  "JavaScriptCore", "bytecode", "CLAUDE_CODE_", "ANTHROPIC_", "2.1.85",
]

function within(size: number, offset: number, length: number) {
  return Number.isSafeInteger(offset) && offset >= 0 && length >= 0 && offset + length <= size
}

function peLayout(header: Buffer) {
  if (header.length < 64 || header.toString("ascii", 0, 2) !== "MZ") throw new Error("Missing DOS header")
  const pe = header.readUInt32LE(0x3c)
  if (!within(header.length, pe, 24) || header.toString("ascii", pe, pe + 4) !== "PE\0\0") throw new Error("Invalid PE signature or offset")
  const count = header.readUInt16LE(pe + 6)
  const optional = pe + 24
  const optionalSize = header.readUInt16LE(pe + 20)
  const sectionsOffset = optional + optionalSize
  if (optionalSize < 2 || !within(header.length, optional, optionalSize) || !within(header.length, sectionsOffset, count * 40)) throw new Error("Truncated PE headers")
  const magic = header.readUInt16LE(optional)
  if (magic !== 0x10b && magic !== 0x20b) throw new Error("Unsupported optional header")
  const dataDirectory = optional + (magic === 0x20b ? 112 : 96)
  const directoryCount = optional + (magic === 0x20b ? 108 : 92)
  if (!within(optional + optionalSize, directoryCount, 4)) throw new Error("Truncated data directories")
  return { pe, count, optional, optionalSize, sectionsOffset, magic, dataDirectory, directoryCount }
}

function peCertificate(header: Buffer, fileSize: number, layout: ReturnType<typeof peLayout>): Range | null {
  const { optional, optionalSize, dataDirectory, directoryCount } = layout
  let certificate: Range | null = null
  if (header.readUInt32LE(directoryCount) > 4) {
    if (!within(optional + optionalSize, dataDirectory + 32, 8)) throw new Error("Truncated security directory")
    const start = header.readUInt32LE(dataDirectory + 32)
    const length = header.readUInt32LE(dataDirectory + 36)
    if (length && !within(fileSize, start, length)) throw new Error("Certificate outside file")
    if (length) certificate = { start, end: start + length }
  }
  return certificate
}

function peSections(header: Buffer, fileSize: number, sectionsOffset: number, count: number) {
  return Array.from({ length: count }, (_, index) => {
    const offset = sectionsOffset + index * 40
    const name = header.toString("ascii", offset, offset + 8).replace(/\0.*$/, "")
    const virtualSize = header.readUInt32LE(offset + 8)
    const virtualAddress = header.readUInt32LE(offset + 12)
    const rawSize = header.readUInt32LE(offset + 16)
    const start = header.readUInt32LE(offset + 20)
    if (rawSize && !within(fileSize, start, rawSize)) throw new Error(`Section ${name} outside file`)
    return { name, virtualSize, virtualAddress, raw: { start, end: start + rawSize } }
  })
}

export function parsePe(header: Buffer, fileSize: number) {
  const layout = peLayout(header)
  const { pe, optional, sectionsOffset, count, magic } = layout
  const certificate = peCertificate(header, fileSize, layout)
  const sections = peSections(header, fileSize, sectionsOffset, count)
  const headersEnd = header.readUInt32LE(optional + 60)
  if (headersEnd > fileSize) throw new Error("Headers outside file")
  const imageEnd = Math.max(headersEnd, sectionsOffset + count * 40, ...sections.filter((section) => section.raw.end > section.raw.start).map((section) => section.raw.end))
  const overlay = imageEnd < fileSize ? { start: imageEnd, end: fileSize } : null
  return { machine: header.readUInt16LE(pe + 4), format: magic === 0x20b ? "PE32+" : "PE32", sections, certificate, overlay }
}

export function readableRuns(data: Buffer, minimum = 4096, base = 0): Range[] {
  const result: Range[] = []
  let start = -1
  for (let i = 0; i <= data.length; i++) {
    const byte = data[i]
    const printable = isPrintable(byte)
    if (printable && start < 0) start = i
    if (!printable && start >= 0) {
      if (i - start >= minimum) result.push({ start: base + start, end: base + i })
      start = -1
    }
  }
  return result
}

function isPrintable(byte: number | undefined) {
  return byte === 9 || byte === 10 || byte === 13 || (byte !== undefined && byte >= 32 && byte <= 126)
}

export function findOffsets(data: Buffer, expression: RegExp, base = 0, ownedLength = data.length) {
  const offsets: number[] = []
  const regex = new RegExp(expression.source, expression.flags.includes("g") ? expression.flags : `${expression.flags}g`)
  for (const match of data.toString("latin1").matchAll(regex)) {
    if (match.index >= ownedLength) break
    offsets.push(base + match.index)
  }
  return offsets
}

export function readAt(fd: number, start: number, length: number) {
  const buffer = Buffer.alloc(length)
  for (let read = 0; read < length;) {
    const count = readSync(fd, buffer, read, length - read, start + read)
    if (!count) throw new Error("Unexpected end of file")
    read += count
  }
  return buffer
}

function sha256(data: Buffer) {
  return createHash("sha256").update(data).digest("hex")
}

function hashRange(fd: number, range: Range) {
  const digest = createHash("sha256")
  for (let position = range.start; position < range.end; position += chunkSize) {
    digest.update(readAt(fd, position, Math.min(chunkSize, range.end - position)))
  }
  return digest.digest("hex")
}

function fingerprint(fd: number, size: number, offset: number, matchLength: number) {
  const start = Math.max(0, offset - 64)
  const end = Math.min(size, offset + matchLength + 64)
  return { range: { start, end }, sha256: sha256(readAt(fd, start, end - start)) }
}

function regexAtomEnd(pattern: string, index: number) {
  const char = pattern[index]
  if (char === "[") {
    const end = pattern.indexOf("]", index + 1)
    if (end <= index + 1) throw new Error("Unclosed or empty regex class")
    return end
  }
  if (char === "\\") {
    if (!pattern[index + 1] || !/[dDsSwW\\.]/.test(pattern[index + 1])) throw new Error("Unsupported regex escape")
    return index + 1
  }
  if ("*+?|()^$]".includes(char)) throw new Error("Regex must be bounded and have no groups or anchors")
  if (char === "{") throw new Error("Quantifier needs an atom")
  return index
}

function regexRepetition(pattern: string, index: number) {
  const match = /^\{(\d+)(?:,(\d+))?\}/.exec(pattern.slice(index + 1))
  if (!match) return { width: 1, end: index }
  const maximum = Number(match[2] ?? match[1])
  if (maximum < Number(match[1])) throw new Error("Invalid regex quantifier")
  return { width: maximum, end: index + match[0].length }
}

function boundedRegexWidth(pattern: string) {
  if (!pattern || pattern.length > 256 || /[^\x00-\x7f]/.test(pattern)) throw new Error("Regex must be bounded ASCII")
  let width = 0
  for (let i = 0; i < pattern.length; i++) {
    const repetition = regexRepetition(pattern, regexAtomEnd(pattern, i))
    width += repetition.width
    i = repetition.end
    if (width > 256) throw new Error("Regex maximum match must be <= 256 bytes")
  }
  return width
}

type Marker = { label: string; regex: RegExp; width: number }

function addMarker(markers: Marker[], seen: Map<string, "literal" | "regex">, marker: Marker, kind: "literal" | "regex") {
  const previous = seen.get(marker.label)
  if (previous && previous !== kind) throw new Error(`Marker label collision: ${marker.label}`)
  if (previous) return
  seen.set(marker.label, kind)
  markers.push(marker)
}

function compileMarkers(literals: string[], regexes: string[]) {
  const markers: Marker[] = []
  const seen = new Map<string, "literal" | "regex">()
  for (const label of literals) {
    if (!label || label.length > 256 || /[^\x00-\x7f]/.test(label)) throw new Error("Literal must be 1..256 ASCII bytes")
    addMarker(markers, seen, { label, regex: new RegExp(label.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "g"), width: Buffer.byteLength(label) }, "literal")
  }
  for (const pattern of regexes) {
    const width = boundedRegexWidth(pattern)
    const regex = new RegExp(pattern, "g")
    if (regex.test("")) throw new Error("Regex must not match empty string")
    addMarker(markers, seen, { label: `regex:${pattern}`, regex, width }, "regex")
  }
  return markers
}

function updateMatches(fd: number, size: number, chunk: Buffer, position: number, owned: number, markers: Marker[], results: Map<string, { count: number; windows: object[] }>) {
  for (const marker of markers) {
    const entry = results.get(marker.label)!
    for (const offset of findOffsets(chunk, marker.regex, position, owned)) {
      entry.count++
      if (entry.windows.length < markerLimit) entry.windows.push({ offset, ...fingerprint(fd, size, offset, marker.width) })
    }
  }
}

function mergeRun(runs: Range[], run: Range) {
  const previous = runs.at(-1)
  if (previous && previous.end >= run.start) previous.end = Math.max(previous.end, run.end)
  else runs.push(run)
}

function scanReadable(chunk: Buffer, position: number, openRun: number, runs: Range[]) {
  for (let i = 0; i < chunk.length; i++) {
    const printable = isPrintable(chunk[i])
    if (printable && openRun < 0) openRun = position + i
    if (!printable && openRun >= 0) {
      if (position + i - openRun >= 4096) mergeRun(runs, { start: openRun, end: position + i })
      openRun = -1
    }
  }
  return openRun
}

function countIdentifiers(bytes: Buffer, owned: number, previous: number | undefined, identifiers: Map<string, number>) {
  const prefix = previous === undefined ? "" : String.fromCharCode(previous)
  const text = prefix + bytes.toString("latin1")
  for (const match of text.matchAll(identifierPattern)) {
    if (match.index >= owned + prefix.length) break
    if (match.index < prefix.length) continue
    identifiers.set(match[0], (identifiers.get(match[0]) ?? 0) + 1)
  }
}

function summarizeIdentifiers(identifiers: Map<string, number>) {
  const groups = Object.fromEntries(prefixes.map((prefix) => {
    const entries = [...identifiers].filter(([name]) => name.startsWith(prefix))
    return [prefix, { distinct: entries.length, occurrences: entries.reduce((total, [, count]) => total + count, 0) }]
  }))
  const top = [...identifiers].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0])).slice(0, 80).map(([name, count]) => ({ name, count }))
  return { distinct: identifiers.size, occurrences: [...identifiers.values()].reduce((a, b) => a + b, 0), groups, top }
}

function summarizeRuns(fd: number, runs: Range[]) {
  const ranked = [...runs].sort((a, b) => b.end - b.start - (a.end - a.start)).slice(0, 30)
  const largestReadableRuns = ranked.map((range) => ({ ...range, bytes: range.end - range.start, sha256: hashRange(fd, range) }))
  return { chunkSize, overlap, readableMinimum: 4096, readableRunCount: runs.length, readableBytes: runs.reduce((total, run) => total + run.end - run.start, 0), largestReadableRuns }
}

export function inspect(file: string, literals = defaultMarkers, regexes: string[] = []) {
  const fd = openSync(file, "r")
  try {
    const size = fstatSync(fd).size
    const pe = parsePe(readAt(fd, 0, Math.min(size, 1024 * 1024)), size)
    const markers = compileMarkers(literals, regexes)
    const matches = new Map(markers.map((marker) => [marker.label, { count: 0, windows: [] as object[] }]))
    const identifiers = new Map<string, number>()
    const runs: Range[] = []
    const digest = createHash("sha256")
    let openRun = -1
    let previous: number | undefined
    for (let position = 0; position < size; position += chunkSize) {
      const owned = Math.min(chunkSize, size - position)
      const bytes = readAt(fd, position, owned + Math.min(overlap, size - position - owned))
      digest.update(bytes.subarray(0, owned))
      updateMatches(fd, size, bytes, position, owned, markers, matches)
      countIdentifiers(bytes, owned, previous, identifiers)
      openRun = scanReadable(bytes.subarray(0, owned), position, openRun, runs)
      previous = bytes[owned - 1]
    }
    if (openRun >= 0 && size - openRun >= 4096) mergeRun(runs, { start: openRun, end: size })
    return {
      file: path.basename(file), size, sha256: digest.digest("hex"), pe,
      scan: summarizeRuns(fd, runs),
      markers: Object.fromEntries(matches),
      identifiers: summarizeIdentifiers(identifiers),
    }
  } finally {
    closeSync(fd)
  }
}

const help = `Usage: bun scripts/inspect-claude.ts --input PATH [--output PATH] [--literal TEXT] [--regex BOUNDED_PATTERN]
       bun scripts/inspect-claude.ts --input PATH --extract START:END --output TEMP_PATH
       bun scripts/inspect-claude.ts --input PATH --probe OFFSET

Manifest scans the entire file; offsets and ranges are decimal, end exclusive. Windows contain hashes, not source text.
--literal can repeat; custom ASCII literals replace defaults. Regex supports ASCII literals, simple classes and bounded {n,m} quantifiers up to 256 bytes.
--extract writes up to 16 MiB of raw bytes to an explicit path under ${tempRoot}; no extracted code is executed.
--probe prints up to 96 bytes at an offset as escaped ASCII, plus its SHA-256. Avoid probing secrets.
--output for manifest is optional; stdout by default. --help prints this message.
`

function options(args: string[]) {
  const result: { input?: string; output?: string; extract?: string; probe?: string; literal: string[]; regex: string[] } = { literal: [], regex: [] }
  const setters: Record<string, (value: string) => void> = {
    "--input": (value) => { result.input = value },
    "--output": (value) => { result.output = value },
    "--extract": (value) => { result.extract = value },
    "--probe": (value) => { result.probe = value },
    "--literal": (value) => { result.literal.push(value) },
    "--regex": (value) => { result.regex.push(value) },
  }
  for (let i = 0; i < args.length; i++) {
    const key = args[i]
    if (key === "--help") return null
    if (!setters[key] || !args[i + 1]) throw new Error(`Invalid argument: ${key}`)
    setters[key](args[++i])
  }
  if (!result.input) throw new Error("--input is required")
  return result
}

function inside(parent: string, root: string) {
  const relative = path.relative(root, parent)
  return relative !== ".." && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative)
}

export function extractDestination(destination: string, root = tempRoot) {
  const realTemp = realpathSync(tmpdir())
  const realRoot = realpathSync(root)
  const parent = realpathSync(path.dirname(path.resolve(destination)))
  if (realRoot === realTemp || !inside(realRoot, realTemp) || !inside(parent, realRoot)) {
    throw new Error("Extraction output must be inside the temp directory")
  }
  return path.join(parent, path.basename(destination))
}

function extract(input: string, destination: string, range: string) {
  const match = /^(\d+):(\d+)$/.exec(range)
  if (!match) throw new Error("Extraction range must be START:END in decimal")
  const start = Number(match[1])
  const end = Number(match[2])
  if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end) || end <= start || end - start > 16 * chunkSize) throw new Error("Extraction requires a valid range <= 16 MiB")
  const output = extractDestination(destination)
  const fd = openSync(input, "r")
  try {
    if (end > fstatSync(fd).size) throw new Error("Extraction range outside file")
    const bytes = readAt(fd, start, end - start)
    writeFileSync(output, bytes, { flag: "wx" })
    console.log(JSON.stringify({ start, end, sha256: sha256(bytes), output: destination }))
  } finally {
    closeSync(fd)
  }
}

function probe(input: string, value: string) {
  const offset = Number(value)
  if (!/^\d+$/.test(value) || !Number.isSafeInteger(offset)) throw new Error("Probe offset must be a decimal integer")
  const fd = openSync(input, "r")
  try {
    const size = fstatSync(fd).size
    if (offset >= size) throw new Error("Probe offset outside file")
    const bytes = readAt(fd, offset, Math.min(96, size - offset))
    const text = [...bytes].map((byte) => byte >= 32 && byte <= 126 ? String.fromCharCode(byte) : `\\x${byte.toString(16).padStart(2, "0")}`).join("")
    console.log(JSON.stringify({ offset, end: offset + bytes.length, sha256: sha256(bytes), ascii: text }))
  } finally {
    closeSync(fd)
  }
}

if (import.meta.main) {
  try {
    const args = options(process.argv.slice(2))
    if (!args) console.log(help)
    else if (args.probe) probe(args.input!, args.probe)
    else if (args.extract) {
      if (!args.output) throw new Error("--extract requires --output")
      extract(args.input!, args.output, args.extract)
    } else {
      const report = JSON.stringify(inspect(args.input!, args.literal.length ? args.literal : defaultMarkers, args.regex), null, 2) + "\n"
      if (args.output) writeFileSync(args.output, report, { flag: "wx" })
      else console.log(report)
    }
  } catch (error) {
    console.error(error instanceof Error ? error.message : error)
    process.exitCode = 1
  }
}
