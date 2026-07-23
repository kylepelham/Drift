import { mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs"
import path from "node:path"

const root = path.resolve(import.meta.dirname, "..")
export const engineUpstream = path.join(root, "engine", "upstream")
const overlayDirectory = path.join(root, "engine", "overlays")
const lockDirectory = path.join(root, "engine", ".overlay-lock")

const overlays = () =>
  readdirSync(overlayDirectory)
    .filter((file) => file.endsWith(".patch"))
    .sort()
    .map((file) => path.join(overlayDirectory, file))

async function gitApply(args: string[], quiet = false) {
  const process = Bun.spawn(["git", "apply", "--directory=engine/upstream", ...args], {
    cwd: root,
    stdout: quiet ? "ignore" : "inherit",
    stderr: quiet ? "ignore" : "inherit",
  })
  return (await process.exited) === 0
}

async function acquireLock() {
  for (let attempt = 0; attempt < 240; attempt++) {
    try {
      mkdirSync(lockDirectory)
      writeFileSync(path.join(lockDirectory, "pid"), `${process.pid}`)
      return
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error
      try {
        const owner = Number(readFileSync(path.join(lockDirectory, "pid"), "utf8"))
        process.kill(owner, 0)
      } catch {
        rmSync(lockDirectory, { recursive: true, force: true })
        continue
      }
      await Bun.sleep(250)
    }
  }
  throw new Error("Timed out waiting for the engine overlay lock")
}

async function restoreAppliedOverlays() {
  for (const overlay of overlays().reverse()) {
    if (await gitApply(["--reverse", "--check", overlay], true)) {
      if (!(await gitApply(["--reverse", overlay]))) throw new Error(`Could not restore ${path.basename(overlay)}`)
    }
  }
}

export async function withEngineOverlays<T>(run: () => Promise<T>) {
  await acquireLock()
  const applied: string[] = []
  try {
    await restoreAppliedOverlays()
    for (const overlay of overlays()) {
      if (!(await gitApply(["--check", overlay], true))) {
        throw new Error(`${path.basename(overlay)} no longer applies; refresh it for the new OpenCode version`)
      }
      if (!(await gitApply([overlay]))) throw new Error(`Could not apply ${path.basename(overlay)}`)
      applied.push(overlay)
    }
    return await run()
  } finally {
    for (const overlay of applied.reverse()) {
      if (!(await gitApply(["--reverse", overlay]))) console.error(`Failed to restore ${path.basename(overlay)}`)
    }
    rmSync(lockDirectory, { recursive: true, force: true })
  }
}
