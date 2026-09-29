import { expect, test } from "bun:test"
import { closeSync, mkdirSync, mkdtempSync, openSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import { extractDestination, findOffsets, inspect, parsePe, readAt, readableRuns } from "../scripts/inspect-claude"

function fixture(sectionEnd = 1024) {
  const bytes = Buffer.alloc(1024)
  bytes.write("MZ")
  bytes.writeUInt32LE(0x80, 0x3c)
  bytes.write("PE\0\0", 0x80)
  bytes.writeUInt16LE(0x8664, 0x84)
  bytes.writeUInt16LE(1, 0x86)
  bytes.writeUInt16LE(240, 0x94)
  bytes.writeUInt16LE(0x20b, 0x98)
  bytes.writeUInt32LE(16, 0x98 + 108)
  bytes.writeUInt32LE(900, 0x98 + 112 + 32)
  bytes.writeUInt32LE(100, 0x98 + 112 + 36)
  bytes.write(".bun", 0x188)
  bytes.writeUInt32LE(512, 0x188 + 16)
  bytes.writeUInt32LE(sectionEnd - 512, 0x188 + 20)
  return bytes
}

test("PE sections, certificate and overlay are file-offset ranges", () => {
  const pe = parsePe(fixture(), 1100)
  expect(pe.format).toBe("PE32+")
  expect(pe.sections[0].raw).toEqual({ start: 512, end: 1024 })
  expect(pe.certificate).toEqual({ start: 900, end: 1000 })
  expect(pe.overlay).toEqual({ start: 1024, end: 1100 })
})

test("rejects corrupt PE headers and out-of-file ranges", () => {
  expect(() => parsePe(Buffer.alloc(64), 1024)).toThrow("DOS")
  const badOffset = fixture()
  badOffset.writeUInt32LE(5000, 0x3c)
  expect(() => parsePe(badOffset, 1100)).toThrow("signature")
  const badSection = fixture(2048)
  expect(() => parsePe(badSection, 1100)).toThrow("outside file")
  expect(() => parsePe(fixture(), 950)).toThrow("Certificate outside file")
  const badHeaders = fixture()
  badHeaders.writeUInt32LE(2000, 0x98 + 60)
  expect(() => parsePe(badHeaders, 1100)).toThrow("Headers outside file")
})

test("PE overlay includes header extent but ignores zero-size section pointers", () => {
  const bytes = fixture()
  bytes.writeUInt32LE(900, 0x98 + 60)
  bytes.writeUInt32LE(0, 0x188 + 16)
  bytes.writeUInt32LE(5000, 0x188 + 20)
  expect(parsePe(bytes, 1100).overlay).toEqual({ start: 900, end: 1100 })
})

test("Latin1 matching returns byte offsets, including across chunk boundaries", () => {
  const bytes = Buffer.from([0xc3, 0xa9, 65, 66, 67, 0, 65, 66, 67])
  expect(findOffsets(bytes, /ABC/g, 73)).toEqual([75, 79])
  expect(findOffsets(bytes, /ABC/g, 73, 5)).toEqual([75])
  expect(readableRuns(bytes, 3, 73)).toEqual([{ start: 75, end: 78 }, { start: 79, end: 82 }])
})

test("stream scan owns matches once and hashes matching source regions", () => {
  const directory = mkdtempSync(path.join(tmpdir(), "drift-inspect-test-"))
  try {
    const bytes = Buffer.alloc(1024 * 1024 + 11000)
    fixture().copy(bytes)
    const source = Buffer.from("// @bun @bytecode @bun-cjs\n" + "A".repeat(5000))
    source.copy(bytes, 1024 * 1024 - 5)
    source.copy(bytes, 1024 * 1024 + 5100)
    const file = path.join(directory, "fixture.exe")
    writeFileSync(file, bytes)
    const result = inspect(file, ["@bytecode"], ["@b[a-z]{7}"])
    expect(result.markers["@bytecode"].count).toBe(2)
    expect(result.markers["regex:@b[a-z]{7}"].count).toBe(2)
    expect(result.markers["@bytecode"].windows[0]).toMatchObject({ offset: 1024 * 1024 + 3 })
    expect(result.scan.largestReadableRuns[0].sha256).toBe(result.scan.largestReadableRuns[1].sha256)
    expect(() => inspect(file, [], ["@b.*"])).toThrow("bounded")
    expect(() => inspect(file, [], ["[A-Z]{3,}"])).toThrow("Quantifier needs an atom")
    expect(() => inspect(file, [], ["[A-Z]{3,300}"])).toThrow("<= 256")
    expect(() => inspect(file, [], ["[A-Z]{8,3}"])).toThrow("Invalid regex quantifier")
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})

test("short reads fail rather than returning partial bytes", () => {
  const directory = mkdtempSync(path.join(tmpdir(), "drift-inspect-read-"))
  try {
    const file = path.join(directory, "short.bin")
    writeFileSync(file, Buffer.from("ABCDE"))
    const fd = openSync(file, "r")
    try {
      expect(readAt(fd, 1, 3).toString()).toBe("BCD")
      expect(() => readAt(fd, 2, 4)).toThrow("Unexpected end of file")
    } finally {
      closeSync(fd)
    }
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})

test("identifier boundary checks use preceding bytes and marker labels are unique", () => {
  const directory = mkdtempSync(path.join(tmpdir(), "drift-inspect-boundary-"))
  try {
    const bytes = Buffer.alloc(2 * 1024 * 1024 + 100)
    fixture().copy(bytes)
    bytes.write(" CLAUDE_CODE_EDGE", 1024 * 1024 - 5)
    bytes.write("xCLAUDE_CODE_BAD", 2 * 1024 * 1024 - 1)
    bytes.write(" CLAUDE_CODE_GOOD EDGE", 2 * 1024 * 1024 + 24)
    const file = path.join(directory, "boundary.exe")
    writeFileSync(file, bytes)
    const report = inspect(file, ["EDGE", "EDGE"], ["GOOD", "GOOD"])
    expect(report.identifiers.groups.CLAUDE_CODE_).toEqual({ distinct: 2, occurrences: 2 })
    expect(report.markers.EDGE.count).toBe(2)
    expect(report.markers["regex:GOOD"].count).toBe(1)
    expect(() => inspect(file, ["regex:GOOD"], ["GOOD"])).toThrow("Marker label collision")
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})

test("extraction checks the real destination parent against its temp root", () => {
  const directory = mkdtempSync(path.join(tmpdir(), "drift-inspect-root-"))
  try {
    const root = path.join(directory, "opencode")
    const outside = path.join(directory, "other")
    mkdirSync(root)
    mkdirSync(outside)
    expect(extractDestination(path.join(root, "output.bin"), root)).toBe(path.join(root, "output.bin"))
    expect(() => extractDestination(path.join(outside, "output.bin"), root)).toThrow("inside the temp directory")
    const link = path.join(root, "escape")
    try {
      symlinkSync(outside, link, process.platform === "win32" ? "junction" : "dir")
      expect(() => extractDestination(path.join(link, "output.bin"), root)).toThrow("inside the temp directory")
      const escapedRoot = path.join(directory, "escaped-root")
      symlinkSync(path.dirname(realpathSync(tmpdir())), escapedRoot, process.platform === "win32" ? "junction" : "dir")
      expect(() => extractDestination(path.join(escapedRoot, "output.bin"), escapedRoot)).toThrow("inside the temp directory")
    } catch (error) {
      if (!(error instanceof Error) || !("code" in error) || !["EPERM", "EACCES"].includes(String(error.code))) throw error
    }
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})
