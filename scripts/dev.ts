import { randomBytes } from "node:crypto"
import { existsSync, rmSync } from "node:fs"
import path from "node:path"
import { engineBinary, engineEnv, prepareRuntime } from "./engine-runtime"

const root = path.resolve(import.meta.dirname, "..")
if (!existsSync(engineBinary)) {
  console.error("drift-engine binary missing. Run: bun run build:engine")
  process.exit(1)
}

// Off 4096 so a dev build runs beside an installed Drift.
const port = process.env.DRIFT_ENGINE_PORT ?? "4196"
const password = process.env.OPENCODE_SERVER_PASSWORD ?? randomBytes(32).toString("hex")
const runtime = await prepareRuntime()
const engine = Bun.spawn([engineBinary, "serve", "--hostname", "127.0.0.1", "--port", port], {
  cwd: root,
  stdout: "inherit",
  stderr: "inherit",
  env: engineEnv(runtime, password),
})
// The native engine runs beside the legacy one until M1; browser dev reaches it through env.
Bun.spawnSync(["cargo", "build", "-q", "-p", "drift-engined"], { cwd: root, stdout: "inherit", stderr: "inherit" })
const native = Bun.spawn([path.join(root, "target", "debug", "drift-engined.exe"), "--data-dir", path.join(runtime, "native")], {
  cwd: root,
  stdout: "pipe",
  stderr: "inherit",
})
const nativeTarget = await readNativeTarget(native.stdout)

const vite = Bun.spawn([process.execPath, "x", "vite"], {
  cwd: root,
  stdout: "inherit",
  stderr: "inherit",
  env: {
    ...process.env,
    VITE_ENGINE_URL: `http://127.0.0.1:${port}`,
    VITE_ENGINE_USERNAME: process.env.OPENCODE_SERVER_USERNAME ?? "opencode",
    VITE_ENGINE_PASSWORD: password,
    VITE_NATIVE_ENGINE_URL: nativeTarget.url,
    VITE_NATIVE_ENGINE_TOKEN: nativeTarget.token,
  },
})

async function readNativeTarget(stdout: ReadableStream<Uint8Array>) {
  const found: Record<string, string> = {}
  let buffered = ""
  for await (const chunk of stdout) {
    buffered += new TextDecoder().decode(chunk)
    for (const line of buffered.split("\n")) {
      const [key, value] = line.trim().split(" ")
      if (key === "url" || key === "token") found[key] = value
    }
    if (found.url && found.token) return { url: found.url, token: found.token }
  }
  throw new Error("drift-engined exited before reporting its address")
}

async function shutdown() {
  engine.kill()
  native.kill()
  vite.kill()
  await Promise.all([engine.exited, native.exited, vite.exited])
  rmSync(runtime, { recursive: true, force: true })
  process.exit(0)
}
process.on("SIGINT", () => void shutdown())
process.on("SIGTERM", () => void shutdown())
await Promise.race([engine.exited, native.exited, vite.exited])
await shutdown()
