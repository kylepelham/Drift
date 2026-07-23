import { $ } from "bun"
import { cpSync, mkdirSync } from "node:fs"
import path from "node:path"
import { withEngineOverlays } from "./engine-overlays"

const root = path.resolve(import.meta.dirname, "..")
const upstream = path.join(root, "engine", "upstream", "packages", "opencode")
const triple = "x86_64-pc-windows-msvc"

await withEngineOverlays(async () => {
  await $`bun run script/build.ts --single --skip-embed-web-ui --skip-install`.cwd(upstream)
})

const built = path.join(upstream, "dist", "opencode-windows-x64", "bin", "opencode.exe")
const out = path.join(root, "src-tauri", "binaries")
mkdirSync(out, { recursive: true })
cpSync(built, path.join(out, `drift-engine-${triple}.exe`))
cpSync(built, path.join(out, "drift-engine.exe"))
console.log("drift-engine ready in src-tauri/binaries")
