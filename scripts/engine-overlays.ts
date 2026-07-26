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
  let missingOwnerAttempts = 0
  for (let attempt = 0; attempt < 240; attempt++) {
    try {
      mkdirSync(lockDirectory)
      writeFileSync(path.join(lockDirectory, "pid"), `${process.pid}`)
      return
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error
      let owner = 0
      try {
        owner = Number(readFileSync(path.join(lockDirectory, "pid"), "utf8"))
      } catch (ownerError) {
        if ((ownerError as NodeJS.ErrnoException).code !== "ENOENT") throw ownerError
      }
      if (!Number.isInteger(owner) || owner <= 0) {
        if (missingOwnerAttempts++ < 3) {
          await Bun.sleep(250)
          continue
        }
        throw new Error(`Invalid engine overlay lock owner; inspect ${lockDirectory} before retrying`)
      }
      missingOwnerAttempts = 0
      try {
        process.kill(owner, 0)
      } catch (ownerError) {
        const code = (ownerError as NodeJS.ErrnoException).code
        if (code === "EPERM") {
          await Bun.sleep(250)
          continue
        }
        if (code !== "ESRCH") throw ownerError
        console.warn(`Recovering engine overlays left by process ${owner}`)
        writeFileSync(path.join(lockDirectory, "pid"), `${process.pid}`)
        return
      }
      await Bun.sleep(250)
    }
  }
  throw new Error("Timed out waiting for the engine overlay lock")
}

function releaseLock() {
  const owner = Number(readFileSync(path.join(lockDirectory, "pid"), "utf8"))
  if (owner !== process.pid) throw new Error(`Engine overlay lock is owned by process ${owner}`)
  rmSync(lockDirectory, { recursive: true })
}

async function upstreamChanges() {
  const child = Bun.spawn(["git", "status", "--porcelain=v1", "--untracked-files=all", "--", "engine/upstream"], {
    cwd: root,
    stdout: "pipe",
    stderr: "inherit",
  })
  const output = await new Response(child.stdout).text()
  if ((await child.exited) !== 0) throw new Error("Could not inspect the engine upstream worktree")
  return output.split(/\r?\n/).filter(Boolean)
}

export interface EngineOverlayOperations {
  overlays: string[]
  acquireLock: () => Promise<void>
  releaseLock: () => void
  canApply: (overlay: string, reverse: boolean) => Promise<boolean>
  apply: (overlay: string, reverse: boolean) => Promise<boolean>
  upstreamChanges: () => Promise<string[]>
}

function aggregate(errors: unknown[], message: string) {
  if (errors.length === 1 && errors[0] instanceof Error) return errors[0]
  return new AggregateError(errors, message)
}

async function inspectClean(operations: EngineOverlayOperations) {
  const failures: Error[] = []
  try {
    const changes = await operations.upstreamChanges()
    if (changes.length > 0) {
      failures.push(new Error(`Engine upstream is not clean after overlay restoration:\n${changes.join("\n")}`))
    }
  } catch (cause) {
    failures.push(new Error("Could not verify the engine upstream worktree is clean", { cause }))
  }
  return failures
}

async function recoverInterruptedOverlays(operations: EngineOverlayOperations) {
  const failures: Error[] = []
  let changes: string[]
  try {
    changes = await operations.upstreamChanges()
  } catch (cause) {
    throw new Error("Could not inspect the engine upstream worktree during startup recovery", { cause })
  }
  if (changes.length === 0) return

  for (const overlay of [...operations.overlays].reverse()) {
    let applied = false
    try {
      applied = await operations.canApply(overlay, true)
    } catch (cause) {
      failures.push(new Error(`Could not inspect applied ${path.basename(overlay)} during startup recovery`, { cause }))
      continue
    }
    if (!applied) continue
    try {
      if (!(await operations.apply(overlay, true))) {
        failures.push(new Error(`Could not recover ${path.basename(overlay)} from an interrupted command`))
      }
    } catch (cause) {
      failures.push(new Error(`Could not recover ${path.basename(overlay)} from an interrupted command`, { cause }))
    }
  }

  try {
    changes = await operations.upstreamChanges()
    if (changes.length > 0) {
      failures.push(
        new Error(`Startup recovery left engine/upstream dirty; manual recovery is required:\n${changes.join("\n")}`),
      )
    }
  } catch (cause) {
    failures.push(new Error("Could not verify startup recovery restored the engine upstream worktree", { cause }))
  }
  if (failures.length > 0) throw aggregate(failures, "Engine overlay startup recovery failed")
}

const defaultOperations = (): EngineOverlayOperations => ({
  overlays: overlays(),
  acquireLock,
  releaseLock,
  canApply: (overlay, reverse) =>
    gitApply(reverse ? ["--reverse", "--check", overlay] : ["--check", overlay], true),
  apply: (overlay, reverse) => gitApply(reverse ? ["--reverse", overlay] : [overlay]),
  upstreamChanges,
})

export async function runWithEngineOverlays<T>(run: () => Promise<T>, operations: EngineOverlayOperations) {
  await operations.acquireLock()
  const applied: string[] = []
  let result: T | undefined
  let operationError: unknown
  let operationFailed = false

  try {
    await recoverInterruptedOverlays(operations)
    for (const overlay of operations.overlays) {
      if (!(await operations.canApply(overlay, false))) {
        throw new Error(`${path.basename(overlay)} no longer applies; refresh it for the new OpenCode version`)
      }
      if (!(await operations.apply(overlay, false))) throw new Error(`Could not apply ${path.basename(overlay)}`)
      applied.push(overlay)
    }
    result = await run()
  } catch (error) {
    operationFailed = true
    operationError = error
  }

  const cleanupFailures: Error[] = []
  for (const overlay of [...applied].reverse()) {
    try {
      if (!(await operations.apply(overlay, true))) {
        cleanupFailures.push(new Error(`Failed to restore ${path.basename(overlay)}`))
      }
    } catch (cause) {
      cleanupFailures.push(new Error(`Failed to restore ${path.basename(overlay)}`, { cause }))
    }
  }

  const cleanlinessFailures = await inspectClean(operations)
  cleanupFailures.push(...cleanlinessFailures)
  if (cleanlinessFailures.length === 0) {
    try {
      operations.releaseLock()
    } catch (cause) {
      cleanupFailures.push(new Error("Failed to release the engine overlay lock", { cause }))
    }
  }

  const cleanupError =
    cleanupFailures.length > 0 ? aggregate(cleanupFailures, "Engine overlay restoration failed") : undefined
  if (operationFailed && cleanupError) {
    throw new AggregateError(
      [operationError, cleanupError],
      "Engine command failed and the upstream subtree was not fully restored",
    )
  }
  if (operationFailed) throw operationError
  if (cleanupError) throw cleanupError
  return result as T
}

export async function withEngineOverlays<T>(run: () => Promise<T>) {
  return runWithEngineOverlays(run, defaultOperations())
}
