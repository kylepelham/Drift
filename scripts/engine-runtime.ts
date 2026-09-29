// Assembles the temporary config directory the legacy opencode engine needs to boot with Drift's plugins.
import { existsSync, mkdtempSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import { promptCatalog } from "./build-extensions"

const root = path.resolve(import.meta.dirname, "..")
export const engineBinary = path.join(root, "src-tauri", "binaries", "drift-engine.exe")
const extensions = path.join(root, "engine", "opencode")

export async function prepareRuntime(extra: Record<string, unknown> = {}) {
  if (!existsSync(path.join(extensions, "node_modules"))) {
    Bun.spawnSync([process.execPath, "install"], { cwd: extensions, stdout: "inherit", stderr: "inherit" })
  }
  const runtime = mkdtempSync(path.join(os.tmpdir(), "drift-engine-config-"))
  const pendingDirectory = path.join(runtime, "pending")
  const sentinelPath = path.join(runtime, "mcp-fail-closed.json")
  const baseConfig = await Bun.file(path.join(extensions, "opencode.json")).json()
  await Bun.write(path.join(runtime, "mcp-approvals.json"), JSON.stringify({ version: 3, generation: 0, decisions: [] }))
  await Bun.write(path.join(runtime, "prompt-catalog.json"), JSON.stringify(promptCatalog()))
  await Bun.write(path.join(runtime, "prompt-overrides.json"), JSON.stringify({ version: 1, families: {} }))
  await Bun.write(
    path.join(runtime, "opencode.json"),
    JSON.stringify({
      ...baseConfig,
      ...extra,
      plugin: [
        ...baseConfig.plugin,
        path.join(extensions, "plugin", "spawn-thread.ts"),
        [path.join(extensions, "plugin", "prompt-overrides.ts"), { catalogPath: path.join(runtime, "prompt-catalog.json"), settingsPath: path.join(runtime, "prompt-overrides.json") }],
        [path.join(extensions, "plugin", "mcp-approval.ts"), { policyPath: path.join(runtime, "mcp-approvals.json"), pendingDirectory, sentinelPath, generation: 0 }],
      ],
    }),
  )
  return runtime
}

export function engineEnv(runtime: string, password: string): Record<string, string> {
  return {
    ...(process.env as Record<string, string>),
    OPENCODE_CONFIG_DIR: runtime,
    OPENCODE_SERVER_PASSWORD: password,
    OPENCODE_SERVER_USERNAME: "opencode",
    DRIFT_MCP_APPROVAL_REQUIRED: "1",
  }
}
