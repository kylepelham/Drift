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
    const approvalPath = path.join(output, "plugin", "mcp-approval.js")
    const promptPath = path.join(output, "plugin", "prompt-overrides.js")
    const source = await Bun.file(pluginPath).text()
    const approvalSource = await Bun.file(approvalPath).text()
    const promptSource = await Bun.file(promptPath).text()
    const manifest = await Bun.file(path.join(output, "package.json")).json()
    expect(source).not.toContain('from"@opencode-ai/plugin"')
    expect(source).not.toContain('from"zod"')
    expect(approvalSource).not.toContain('from"@opencode-ai/plugin"')
    expect(promptSource).not.toContain('from"@opencode-ai/plugin"')
    expect(manifest.dependencies).toBeUndefined()
    expect(typeof (await import(pathToFileURL(pluginPath).href)).SpawnThread).toBe("function")
    const approval = await import(pathToFileURL(approvalPath).href)
    expect(typeof approval.McpApproval).toBe("function")
    expect(Object.values(approval).filter((value) => typeof value === "function")).toHaveLength(1)
    const prompt = await import(pathToFileURL(promptPath).href)
    expect(typeof prompt.PromptOverrides).toBe("function")
    const catalog = await Bun.file(path.join(output, "prompt-catalog.json")).json()
    expect(catalog.families).toHaveLength(9)
    const gpt = catalog.families.find((item: { id: string }) => item.id === "gpt")
    expect(gpt.default).toStartWith("You are Drift")
    const settingsPath = path.join(output, "prompt-overrides.json")
    await Bun.write(settingsPath, JSON.stringify({ version: 1, families: {} }))
    const hooks = await prompt.PromptOverrides({} as never, {
      catalogPath: path.join(output, "prompt-catalog.json"),
      settingsPath,
    })
    const system = { system: [`${gpt.original}\nworkspace context`] }
    await hooks["experimental.chat.system.transform"]?.({ model: { api: { id: "gpt-5.4" } } } as never, system)
    expect(system.system[0]).toStartWith("You are Drift")
    expect(system.system[0]).toEndWith("workspace context")
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
