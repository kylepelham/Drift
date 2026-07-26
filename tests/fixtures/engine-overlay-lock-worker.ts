import { appendFileSync, readdirSync, writeFileSync } from "node:fs"
import path from "node:path"
import { acquireEngineOverlayLock, releaseEngineOverlayLock } from "../../scripts/engine-overlays"

interface Input {
  lock: string
  acquired: string
  ready: string
  synchronizeDeadCheck?: string
  holdMs: number
}

const input = JSON.parse(process.argv[2]) as Input

if (input.synchronizeDeadCheck) {
  const kill = process.kill.bind(process)
  Object.defineProperty(process, "kill", {
    value: (...args: Parameters<typeof process.kill>) => {
      try {
        return kill(...args)
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== "ESRCH") throw error
        writeFileSync(path.join(input.synchronizeDeadCheck, String(process.pid)), "")
        while (readdirSync(input.synchronizeDeadCheck!).length < 2) {
          Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 10)
        }
        throw error
      }
    },
  })
}

await acquireEngineOverlayLock(input.lock)
appendFileSync(input.acquired, `${process.pid}\n`)
writeFileSync(input.ready, "")
await Bun.sleep(input.holdMs)
releaseEngineOverlayLock(input.lock)
