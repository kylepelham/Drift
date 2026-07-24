import { expect, test } from "bun:test"
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import { pathToFileURL } from "node:url"
import { buildExtensions } from "../scripts/build-extensions"

test("release extensions load without workspace node_modules", async () => {
  const output = mkdtempSync(path.join(tmpdir(), "drift-extensions-"))
  try {
    await buildExtensions(output)
    const pluginPath = path.join(output, "plugin", "spawn-thread.js")
    const source = await Bun.file(pluginPath).text()
    const manifest = await Bun.file(path.join(output, "package.json")).json()
    expect(source).not.toContain('from"@opencode-ai/plugin"')
    expect(source).not.toContain('from"zod"')
    expect(manifest.dependencies).toBeUndefined()
    expect(typeof (await import(pathToFileURL(pluginPath).href)).SpawnThread).toBe("function")
  } finally {
    rmSync(output, { recursive: true, force: true })
  }
})

test("release extension build removes stale raw resources", async () => {
  const root = mkdtempSync(path.join(tmpdir(), "drift-release-extensions-"))
  const output = path.join(root, "generated")
  const release = path.join(root, "release", "drift-extensions")
  try {
    mkdirSync(path.join(release, "plugin"), { recursive: true })
    writeFileSync(path.join(release, "plugin", "spawn-thread.ts"), "stale")
    await buildExtensions(output, release)
    expect(await Bun.file(path.join(release, "plugin", "spawn-thread.ts")).exists()).toBe(false)
    expect(await Bun.file(path.join(output, "plugin", "spawn-thread.js")).exists()).toBe(true)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
