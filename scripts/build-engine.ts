import { $ } from "bun"
import { cpSync, mkdirSync } from "node:fs"
import path from "node:path"
import { withEngineOverlays } from "./engine-overlays"

const root = path.resolve(import.meta.dirname, "..")
const upstream = path.join(root, "engine", "upstream", "packages", "opencode")
const triple = "x86_64-pc-windows-msvc"
const enginePackage = await Bun.file(path.join(upstream, "package.json")).json()
const engineVersion = typeof enginePackage.version === "string" ? enginePackage.version : "1.0.0"

await withEngineOverlays(async () => {
  await $`bun run script/build.ts --single --skip-embed-web-ui --skip-install`.cwd(upstream).env({
    ...process.env,
    OPENCODE_VERSION: engineVersion,
    OPENCODE_CHANNEL: "latest",
  })
})

const built = path.join(upstream, "dist", "opencode-windows-x64", "bin", "opencode.exe")
const out = path.join(root, "src-tauri", "binaries")
mkdirSync(out, { recursive: true })
cpSync(built, path.join(out, `drift-engine-${triple}.exe`))
cpSync(built, path.join(out, "drift-engine.exe"))
console.log("drift-engine ready in src-tauri/binaries")
