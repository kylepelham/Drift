import { expect, test } from "bun:test"
import type { Message, ToolPart } from "@opencode-ai/sdk/client"
import { nextUserMessage, previousUserMessage, type MessageEntry } from "../src/engine/store"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

test("pluginPaths keeps local JavaScript modules only", async () => {
  const { pluginPaths } = await import("../src/plugins")
  expect(
    pluginPaths(
      JSON.stringify({
        plugins: ["plugins/local.mjs", "nested\\plugin.js", "../outside.mjs", "C:\\outside.js", 3],
      }),
    ),
  ).toEqual(["plugins/local.mjs", "nested/plugin.js"])
})

test("patchFiles reads per-file apply_patch metadata", async () => {
  const { patchFiles } = await import("../src/ui/parts")
  const files = [
    {
      filePath: "S:/Personal/Drift/src/app.tsx",
      relativePath: "src/app.tsx",
      type: "update",
      patch: "@@ -1 +1 @@\n-old\n+new",
      additions: 1,
      deletions: 1,
    },
    {
      filePath: "S:/Personal/Drift/src/new.ts",
      relativePath: "src/new.ts",
      type: "add",
      patch: "@@ -0,0 +1 @@\n+new",
      additions: 1,
      deletions: 0,
    },
  ]
  const part = { state: { status: "completed", metadata: { files } } } as unknown as ToolPart
  expect(patchFiles(part)).toEqual(files)
})

test("tool context actions compose wildcard and tool providers with cleanup", async () => {
  const { registerToolContextActions, toolContextActions } = await import("../src/tool-actions")
  const part = { tool: "custom", state: { status: "completed", input: {} } } as unknown as ToolPart
  const offAny = registerToolContextActions("*", () => ({ id: "any", label: "Any", run: () => undefined }))
  const offTool = registerToolContextActions("custom", () => [
    { id: "one", label: "One", run: () => undefined },
    { id: "two", label: "Two", run: () => undefined },
  ])
  expect(toolContextActions(part).map((action) => action.id)).toEqual(["any", "one", "two"])
  offAny()
  offTool()
  expect(toolContextActions(part)).toEqual([])
})

test("file tool actions resolve changed lines and patch targets", async () => {
  const { builtinFileTargets, firstChangedLine } = await import("../src/tool-actions")
  expect(firstChangedLine("@@ -10,3 +20,4 @@\n context\n-old\n+new")).toBe(21)
  const edit = {
    tool: "edit",
    state: {
      status: "completed",
      input: { filePath: "src/app.tsx" },
      metadata: { diff: "@@ -4 +7 @@\n-old\n+new" },
    },
  } as unknown as ToolPart
  expect(builtinFileTargets(edit, "S:\\Personal\\Drift")).toEqual([
    { path: "S:\\Personal\\Drift\\src/app.tsx", label: "src/app.tsx", line: 7 },
  ])
  const patch = {
    tool: "apply_patch",
    state: {
      status: "completed",
      input: {},
      metadata: {
        files: [
          {
            filePath: "S:/Personal/Drift/old.ts",
            movePath: "S:/Personal/Drift/new.ts",
            relativePath: "new.ts",
            type: "move",
            patch: "@@ -1 +3 @@\n-old\n+new",
          },
          { filePath: "S:/Personal/Drift/gone.ts", relativePath: "gone.ts", type: "delete", patch: "" },
        ],
      },
    },
  } as unknown as ToolPart
  expect(builtinFileTargets(patch, "S:/Personal/Drift")).toEqual([
    { path: "S:/Personal/Drift/new.ts", label: "new.ts", line: 3 },
  ])
})

test("undo and redo move one user prompt at a time", () => {
  const entry = (id: string, role: "user" | "assistant") => ({ info: { id, role } as Message, parts: [] })
  const entries: MessageEntry[] = [entry("01", "user"), entry("02", "assistant"), entry("03", "user")]
  expect(previousUserMessage(entries)?.info.id).toBe("03")
  expect(previousUserMessage(entries, "03")?.info.id).toBe("01")
  expect(nextUserMessage(entries, "01")?.info.id).toBe("03")
})
