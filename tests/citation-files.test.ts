import { expect, test } from "bun:test"
import type { Part } from "@opencode-ai/sdk/client"
import type { EngineState, MessageEntry } from "../src/engine/store"
import { citationFileGroups } from "../src/ui/citation-files"
import { resolveMarkdownCitation } from "../src/ui/markdown-links"

function tool(id: string, name: string, input: Record<string, unknown>, metadata: Record<string, unknown> = {}): Part {
  return { id, type: "tool", sessionID: "session", messageID: "response", tool: name,
    state: { status: "completed", input, metadata, output: "", title: "", time: { start: 1, end: 2 } },
  } as Part
}

function message(id: string, role: "user" | "assistant", created: number, parts: Part[] = []): MessageEntry {
  return { info: { id, sessionID: "session", role, time: { created } }, parts } as MessageEntry
}

function state(entries: MessageEntry[]): Pick<EngineState, "sessions" | "transcripts"> {
  return { sessions: { session: { directory: "C:/" } }, transcripts: { session: entries } } as never
}

const citation = { id: "citation", messageID: "response", sessionID: "session", type: "text", text: "See AmazingCode.cs:345:21" } as Part

test("a drive-root citation resolves the current task's file rather than older or later work", () => {
  const value = state([
    message("old-user", "user", 1),
    message("old-response", "assistant", 2, [tool("old", "read", { filePath: "C:/Other/AmazingCode.cs" })]),
    message("current-user", "user", 3),
    message("read-response", "assistant", 4, [tool("read", "read", { filePath: "C:/Projects/App/AmazingCode.cs" })]),
    message("response", "assistant", 5, [citation, tool("later-part", "read", { filePath: "C:/Later/AmazingCode.cs" })]),
    message("future-user", "user", 6),
    message("future-response", "assistant", 7, [tool("future", "read", { filePath: "C:/Future/AmazingCode.cs" })]),
  ])
  value.transcripts.other = [message("unrelated", "assistant", 4, [tool("other", "read", { filePath: "C:/Elsewhere/AmazingCode.cs" })])]
  const groups = citationFileGroups(value, "session", "response", "citation")
  expect(groups).toEqual([["C:/Projects/App/AmazingCode.cs"], ["C:/Other/AmazingCode.cs"]])
  expect(resolveMarkdownCitation("AmazingCode.cs:345:21", "C:/", groups)).toEqual({
    kind: "file", path: "C:/Projects/App/AmazingCode.cs", line: 345, column: 21,
  })
  expect(citationFileGroups(value, "session", undefined, undefined, 5)).toEqual([
    ["C:/Projects/App/AmazingCode.cs", "C:/Later/AmazingCode.cs"], ["C:/Other/AmazingCode.cs"],
  ])
})

test("collects successful reads, edits, writes, patch moves and attachments without decoding native paths", () => {
  const failed = tool("failed", "read", { filePath: "C:/Missing.cs" })
  if (failed.type === "tool") failed.state = { status: "error", input: failed.state.input, error: "missing", time: { start: 1, end: 2 } }
  const value = state([message("response", "assistant", 1, [
    tool("read", "read", { filePath: "C:\\Project\\100% notes#1.cs" }),
    tool("edit", "edit", { filePath: "Project/Edit.cs" }, { filediff: { file: "C:/Project/Edit.cs" } }),
    tool("write", "write", { filePath: "C:/Project/New.cs" }),
    tool("patch", "apply_patch", {}, { files: [
      { filePath: "C:/Project/Before.cs", movePath: "C:/Project/After.cs" },
      { filePath: "C:/Project/Deleted.cs", type: "delete" },
    ] }),
    tool("patch-input", "apply_patch", { patchText: "*** Begin Patch\n*** Add File: Project/Added.cs\n+x\n*** End Patch" }),
    { id: "attachment", type: "file", sessionID: "session", messageID: "response", url: "file:///C:/Project/Attached%23file.cs", mime: "text/plain" },
    failed, citation,
  ])])
  expect(citationFileGroups(value, "session", "response", "citation")[0]).toEqual([
    "C:/Project/100% notes#1.cs", "C:/Project/Edit.cs", "C:/Project/New.cs", "C:/Project/After.cs",
    "C:/Project/Added.cs", "C:/Project/Attached#file.cs",
  ])
  expect(citationFileGroups(value, "unknown", "response")).toEqual([])
  expect(citationFileGroups(value, "session", "missing")).toEqual([])
  expect(citationFileGroups(value, "session", "response", "missing")).toEqual([])
})

test("delegated result context excludes tools that completed after the result", () => {
  const late = tool("late", "read", { filePath: "C:/Later/AmazingCode.cs" })
  if (late.type === "tool" && late.state.status === "completed") late.state.time.end = 10
  const value = state([message("response", "assistant", 1, [
    tool("read", "read", { filePath: "C:/Projects/App/AmazingCode.cs" }), late,
  ])])
  expect(citationFileGroups(value, "session", undefined, undefined, 5)).toEqual([
    ["C:/Projects/App/AmazingCode.cs"], [],
  ])
})

test.each([
  ["AmazingCode.cs#L345C21", "C:/", "C:/Projects/App/AmazingCode.cs", 345, 21],
  ["AmazingCode.cs:345:21", "C:/", "C:/Projects/App/AmazingCode.cs", 345, 21],
  ["./AmazingCode.cs:345", "C:/", "C:/Projects/App/AmazingCode.cs", 345, undefined],
  ["app/amazingcode.cs#L2", "c:\\", "C:/Projects/App/AmazingCode.cs", 2, undefined],
  ["C:/Exact/AmazingCode.cs:3:4", "C:/", "C:/Exact/AmazingCode.cs", 3, 4],
  ["file:///C:/Exact/AmazingCode.cs#L3", "C:/", "C:/Exact/AmazingCode.cs", 3, undefined],
  ["Missing.cs#L2", "C:/", "C:/Missing.cs", 2, undefined],
  ["../AmazingCode.cs#L2", "C:/Projects", "C:/AmazingCode.cs", 2, undefined],
] as const)("resolves citation %s with its position", (href, directory, path, line, column) => {
  const result = resolveMarkdownCitation(href, directory, [["C:/Projects/App/AmazingCode.cs"]])
  expect(result).toMatchObject({ kind: "file", path })
  if (result.kind !== "file") throw new Error("Expected a file")
  expect(result.line).toBe(line)
  expect(result.column).toBe(column)
})

test("requires an unambiguous suffix and respects directory boundaries and Windows case folding", () => {
  expect(() => resolveMarkdownCitation("Amazing.cs#L1", "C:/", [["C:/One/Amazing.cs", "C:/Two/Amazing.cs"]]))
    .toThrow("Ambiguous file link")
  expect(resolveMarkdownCitation("One/Amazing.cs#L1", "C:/", [["C:/App/One/Amazing.cs", "C:/App/Two/Amazing.cs"]]))
    .toEqual({ kind: "file", path: "C:/App/One/Amazing.cs", line: 1 })
  expect(resolveMarkdownCitation("Amazing.cs", "C:/", [["C:/One/Amazing.cs", "c:/one/amazing.cs"]]))
    .toEqual({ kind: "file", path: "c:/one/amazing.cs" })
  expect(resolveMarkdownCitation("Amazing.cs", "C:/", [["C:/One/Amazing.cs", "C:/Amazing.cs"]]))
    .toEqual({ kind: "file", path: "C:/Amazing.cs" })
  expect(resolveMarkdownCitation("Amazing.cs", "C:/Work", [["C:/WorkElsewhere/Amazing.cs", "D:/Work/Amazing.cs"]]))
    .toEqual({ kind: "file", path: "C:/Work/Amazing.cs" })
  expect(resolveMarkdownCitation("Amazing.cs", "/work", [["/work/project/amazing.cs"]]))
    .toEqual({ kind: "file", path: "/work/Amazing.cs" })
  expect(resolveMarkdownCitation("Amazing.cs", "C:/", [[], ["C:/Earlier/Amazing.cs"]]))
    .toEqual({ kind: "file", path: "C:/Earlier/Amazing.cs" })
})

test.each(["javascript:alert(1)", "javascript:fake.cs:3", "C:fake.cs:3", "bad%0A.cs:3", "bad%2Ffile.cs:3"])("does not turn %s into a file citation", (raw) => {
  expect(resolveMarkdownCitation(raw, "C:/", [["C:/Project/file.cs"]])).toEqual({ kind: "unsupported" })
})

test("keeps external URLs and encoded native filename punctuation intact", () => {
  expect(resolveMarkdownCitation("https://example.com/file.cs:3:4", "C:/")).toEqual({ kind: "external", url: "https://example.com/file.cs:3:4" })
  expect(resolveMarkdownCitation("100%25%20notes%231.cs:7:2", "C:/", [["C:/Project/100% notes#1.cs"]]))
    .toEqual({ kind: "file", path: "C:/Project/100% notes#1.cs", line: 7, column: 2 })
})
