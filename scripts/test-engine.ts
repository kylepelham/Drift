import path from "node:path"
import { engineUpstream, withEngineOverlays } from "./engine-overlays"

async function run(directory: string, args: string[]) {
  const child = Bun.spawn(["bun", "test", "--timeout", "20000", ...args], {
    cwd: path.join(engineUpstream, directory),
    stdin: "inherit",
    stdout: "inherit",
    stderr: "inherit",
  })
  const exitCode = await child.exited
  if (exitCode !== 0) throw new Error(`Engine tests failed in ${directory}`)
}

await withEngineOverlays(async () => {
  await run("packages/core", ["test/move-session.test.ts"])
  await run("packages/opencode", [
    "test/session/compaction.test.ts",
    "-t",
    "continues from compacted context|uses a hidden continuation",
  ])
  await run("packages/opencode", [
    "test/session/processor-effect.test.ts",
    "-t",
    "compact on structured context overflow",
  ])
})
