// Every gate a change must pass: the engine binary first, then the Rust and bun gates side by side. Prints only failures.
import { resolve } from "node:path"

type Step = { name: string; cmd: string[] }
type Proc = ReturnType<typeof Bun.spawn>

const root = resolve(import.meta.dir, "..")
const shownLines = 150
// Conformance tests drive target/debug/drift-engined, so it is built before anything runs against it.
const build: Step = { name: "build drift-engined", cmd: ["cargo", "build", "-q", "-p", "drift-engined"] }
const chains: Step[][] = [
  [
    { name: "engine client", cmd: ["bun", "scripts/gen-engine-client.ts", "--check"] },
    { name: "clippy", cmd: ["cargo", "clippy", "-q", "--workspace", "--all-targets", "--", "-D", "warnings"] },
    { name: "cargo test", cmd: ["cargo", "test", "-q", "--workspace"] },
  ],
  [
    { name: "typecheck", cmd: ["bun", "run", "typecheck"] },
    { name: "bun test", cmd: ["bun", "run", "test"] },
  ],
]

// Windows antivirus briefly holds a freshly linked binary or its .pdb; the link fails with one of these, never because of the code.
const fileLocked = /LNK1104|LNK1201|os error 32/

const running = new Set<Proc>()
let failed = false

async function attempt(step: Step) {
  const proc = Bun.spawn(step.cmd, { cwd: root, stdout: "pipe", stderr: "pipe" })
  running.add(proc)
  const [out, err, code] = await Promise.all([new Response(proc.stdout).text(), new Response(proc.stderr).text(), proc.exited])
  running.delete(proc)
  return { code, output: (out + err).trimEnd() }
}

async function run(step: Step) {
  if (failed) return false
  const started = performance.now()
  let result = await attempt(step)
  if (result.code !== 0 && !failed && fileLocked.test(result.output)) {
    console.log(`retry ${step.name}: a build file was locked`)
    result = await attempt(step)
  }
  if (failed) return false
  const took = `${((performance.now() - started) / 1000).toFixed(1)}s`
  if (result.code === 0) {
    console.log(`ok    ${step.name} (${took})`)
    return true
  }
  failed = true
  for (const other of running) stop(other)
  console.log(`FAIL  ${step.name} (${took})\n${result.output.split("\n").slice(-shownLines).join("\n")}`)
  return false
}

async function chain(steps: Step[]) {
  for (const step of steps) if (!(await run(step))) return false
  return true
}

/** The whole process tree: cargo leaves rustc and test binaries running if only it is killed. */
function stop(proc: Proc) {
  if (process.platform === "win32") Bun.spawnSync(["taskkill", "/T", "/F", "/PID", String(proc.pid)], { stdout: "ignore", stderr: "ignore" })
  else proc.kill()
}

const started = performance.now()
const passed = (await run(build)) && (await Promise.all(chains.map(chain))).every(Boolean)
console.log(`${passed ? "all gates passed" : "gates failed"} in ${((performance.now() - started) / 1000).toFixed(1)}s`)
process.exit(passed ? 0 : 1)
