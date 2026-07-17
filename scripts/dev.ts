import { existsSync } from "node:fs"
import path from "node:path"

const root = path.resolve(import.meta.dirname, "..")
const binary = path.join(root, "src-tauri", "binaries", "drift-engine.exe")
if (!existsSync(binary)) {
  console.error("drift-engine binary missing. Run: bun run build:engine")
  process.exit(1)
}

const port = process.env.DRIFT_ENGINE_PORT ?? "4096"
const engine = Bun.spawn([binary, "serve", "--port", port], {
  cwd: root,
  stdout: "inherit",
  stderr: "inherit",
})
const vite = Bun.spawn([process.execPath, "x", "vite"], {
  cwd: root,
  stdout: "inherit",
  stderr: "inherit",
  env: {
    ...process.env,
    VITE_ENGINE_URL: `http://127.0.0.1:${port}`,
    VITE_ENGINE_USERNAME: process.env.OPENCODE_SERVER_USERNAME ?? "opencode",
    VITE_ENGINE_PASSWORD: process.env.OPENCODE_SERVER_PASSWORD ?? "",
  },
})

function shutdown() {
  engine.kill()
  vite.kill()
  process.exit(0)
}
process.on("SIGINT", shutdown)
process.on("SIGTERM", shutdown)
await Promise.race([engine.exited, vite.exited])
shutdown()
