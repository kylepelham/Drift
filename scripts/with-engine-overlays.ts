import path from "node:path"
import { engineUpstream, withEngineOverlays } from "./engine-overlays"

const [workingDirectory, ...command] = Bun.argv.slice(2)
if (!workingDirectory || command.length === 0) {
  throw new Error("Usage: bun scripts/with-engine-overlays.ts <upstream-directory> <command> [...args]")
}

const exitCode = await withEngineOverlays(async () => {
  const child = Bun.spawn(command, {
    cwd: path.join(engineUpstream, workingDirectory),
    stdin: "inherit",
    stdout: "inherit",
    stderr: "inherit",
  })
  return child.exited
})

process.exit(exitCode)
