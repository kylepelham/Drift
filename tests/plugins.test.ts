import { expect, test } from "bun:test"
import type { ToolPart } from "@opencode-ai/sdk/client"

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
