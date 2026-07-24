import { cpSync, mkdirSync, rmSync } from "node:fs"
import path from "node:path"

const root = path.resolve(import.meta.dirname, "..")
const source = path.join(root, "engine", "opencode")
const defaultOutput = path.join(root, "src-tauri", "generated", "drift-extensions")

export async function buildExtensions(output = defaultOutput, staleReleaseOutput?: string) {
  const plugins = path.join(output, "plugin")
  if (staleReleaseOutput) rmSync(staleReleaseOutput, { recursive: true, force: true })
  mkdirSync(output, { recursive: true })
  rmSync(plugins, { recursive: true, force: true })
  rmSync(path.join(output, "opencode.json"), { force: true })
  rmSync(path.join(output, "package.json"), { force: true })
  mkdirSync(plugins, { recursive: true })
  const result = await Bun.build({
    entrypoints: [path.join(source, "plugin", "spawn-thread.ts"), path.join(source, "plugin", "mcp-approval.ts")],
    outdir: plugins,
    target: "bun",
    format: "esm",
    minify: true,
  })
  if (!result.success) throw new AggregateError(result.logs, "failed to bundle Drift engine extensions")
  cpSync(path.join(source, "opencode.json"), path.join(output, "opencode.json"))
  await Bun.write(
    path.join(output, "package.json"),
    `${JSON.stringify({ name: "drift-engine-extensions", private: true, type: "module" }, null, 2)}\n`,
  )
  return output
}

export function releaseExtensionsPath(targetDir = process.env.CARGO_TARGET_DIR) {
  const cargoTarget = targetDir
    ? path.resolve(root, "src-tauri", targetDir)
    : path.join(root, "src-tauri", "target")
  return path.join(cargoTarget, "release", "drift-extensions")
}

if (import.meta.main) {
  const staleReleaseOutput = process.argv.includes("--clean-release") ? releaseExtensionsPath() : undefined
  const output = await buildExtensions(defaultOutput, staleReleaseOutput)
  console.log(`drift extensions ready in ${path.relative(root, output)}`)
}
