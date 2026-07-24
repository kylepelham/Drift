import { randomBytes } from "node:crypto"
import { existsSync, mkdtempSync, rmSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import { promptCatalog } from "./build-extensions"

const root = path.resolve(import.meta.dirname, "..")
const binary = path.join(root, "src-tauri", "binaries", "drift-engine.exe")
if (!existsSync(binary)) {
  console.error("drift-engine binary missing. Run: bun run build:engine")
  process.exit(1)
}

const extensions = path.join(root, "engine", "opencode")
if (!existsSync(path.join(extensions, "node_modules"))) {
  Bun.spawnSync([process.execPath, "install"], { cwd: extensions, stdout: "inherit", stderr: "inherit" })
}

const port = process.env.DRIFT_ENGINE_PORT ?? "4096"
const password = process.env.OPENCODE_SERVER_PASSWORD ?? randomBytes(32).toString("hex")
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
    plugin: [
      ...baseConfig.plugin,
      path.join(extensions, "plugin", "spawn-thread.ts"),
      [path.join(extensions, "plugin", "prompt-overrides.ts"), { catalogPath: path.join(runtime, "prompt-catalog.json"), settingsPath: path.join(runtime, "prompt-overrides.json") }],
      [path.join(extensions, "plugin", "mcp-approval.ts"), { policyPath: path.join(runtime, "mcp-approvals.json"), pendingDirectory, sentinelPath, generation: 0 }],
    ],
  }),
)
const engine = Bun.spawn([binary, "serve", "--hostname", "127.0.0.1", "--port", port], {
  cwd: root,
  stdout: "inherit",
  stderr: "inherit",
  env: {
    ...process.env,
    OPENCODE_CONFIG_DIR: runtime,
    OPENCODE_SERVER_PASSWORD: password,
    OPENCODE_SERVER_USERNAME: "opencode",
    DRIFT_MCP_APPROVAL_REQUIRED: "1",
  },
})
const vite = Bun.spawn([process.execPath, "x", "vite"], {
  cwd: root,
  stdout: "inherit",
  stderr: "inherit",
  env: {
    ...process.env,
    VITE_ENGINE_URL: `http://127.0.0.1:${port}`,
    VITE_ENGINE_USERNAME: process.env.OPENCODE_SERVER_USERNAME ?? "opencode",
    VITE_ENGINE_PASSWORD: password,
  },
})

function shutdown() {
  engine.kill()
  vite.kill()
  rmSync(runtime, { recursive: true, force: true })
  process.exit(0)
}
process.on("SIGINT", shutdown)
process.on("SIGTERM", shutdown)
await Promise.race([engine.exited, vite.exited])
shutdown()
