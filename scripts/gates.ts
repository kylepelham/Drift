// Conformance tests need the engine binary before the Rust and frontend checks run in parallel.
import { resolve } from "node:path";

type Step = { name: string; cmd: string[] };
type Proc = ReturnType<typeof Bun.spawn>;

const root = resolve(import.meta.dir, "..");
const shownLines = 150;
// Conformance tests drive target/debug/drift-engined, so it is built before anything runs against it.
const build: Step = { name: "build drift-engined", cmd: ["cargo", "build", "-q", "-p", "drift-engined"] };
const chains: Step[][] = [
    [
        { name: "engine client", cmd: ["bun", "scripts/gen-engine-client.ts", "--check"] },
        { name: "clippy", cmd: ["cargo", "clippy", "-q", "--workspace", "--all-targets", "--", "-D", "warnings"] },
        { name: "cargo test", cmd: ["cargo", "test", "-q", "--workspace"] },
    ],
    [
        { name: "format", cmd: ["bun", "run", "format:check"] },
        { name: "lint", cmd: ["bun", "run", "lint"] },
        { name: "typecheck", cmd: ["bun", "run", "typecheck"] },
        { name: "bun test", cmd: ["bun", "run", "test"] },
    ],
];

// Antivirus can briefly lock newly linked binaries or debug symbols, so these errors permit one retry.
const fileLocked = /LNK1104|LNK1201|os error 32/;

const running = new Set<Proc>();
let failed = false;

async function attempt(step: Step) {
    const proc = Bun.spawn(step.cmd, { cwd: root, stdout: "pipe", stderr: "pipe" });
    running.add(proc);

    const [out, err, code] = await Promise.all([
        new Response(proc.stdout).text(),
        new Response(proc.stderr).text(),
        proc.exited,
    ]);
    running.delete(proc);

    return { code, output: (out + err).trimEnd() };
}

async function run(step: Step) {
    if (failed) return false;

    const started = performance.now();
    let result = await attempt(step);
    if (result.code !== 0 && !failed && fileLocked.test(result.output)) {
        console.log(`retry ${step.name}: a build file was locked`);
        result = await attempt(step);
    }
    if (failed) return false;

    const took = `${((performance.now() - started) / 1000).toFixed(1)}s`;
    if (result.code === 0) {
        console.log(`ok    ${step.name} (${took})`);
        return true;
    }

    failed = true;
    for (const other of running) stop(other);
    const failureTail = result.output.split("\n").slice(-shownLines).join("\n");
    console.log(`FAIL  ${step.name} (${took})\n${failureTail}`);

    return false;
}

async function chain(steps: Step[]) {
    for (const step of steps) if (!(await run(step))) return false;
    return true;
}

/** Stops the whole process tree because cargo can leave compiler and test processes running. */
function stop(proc: Proc) {
    if (process.platform === "win32")
        Bun.spawnSync(["taskkill", "/T", "/F", "/PID", String(proc.pid)], { stdout: "ignore", stderr: "ignore" });
    else proc.kill();
}

const started = performance.now();
const passed = (await run(build)) && (await Promise.all(chains.map(chain))).every(Boolean);
console.log(`${passed ? "all gates passed" : "gates failed"} in ${((performance.now() - started) / 1000).toFixed(1)}s`);
process.exit(passed ? 0 : 1);
