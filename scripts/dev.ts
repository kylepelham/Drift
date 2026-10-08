// Browser dev loop: a headless engine (drift-engined) on a scratch data directory, and Vite pointed at it.
import { mkdtempSync, rmSync } from "node:fs";
import path from "node:path";
import os from "node:os";

const root = path.resolve(import.meta.dirname, "..");
const runtime = mkdtempSync(path.join(os.tmpdir(), "drift-dev-"));
Bun.spawnSync(["cargo", "build", "-q", "-p", "drift-engined"], { cwd: root, stdout: "inherit", stderr: "inherit" });
// Run a copy so cargo can rebuild the real binary while dev is up.
const binary = path.join(runtime, "drift-engined.exe");
await Bun.write(binary, Bun.file(path.join(root, "target", "debug", "drift-engined.exe")));
const engine = Bun.spawn([binary, "--data-dir", path.join(runtime, "data")], {
    cwd: root,
    stdout: "pipe",
    stderr: "inherit",
});
const target = await readTarget(engine.stdout);

const vite = Bun.spawn([process.execPath, "x", "vite"], {
    cwd: root,
    stdout: "inherit",
    stderr: "inherit",
    env: { ...process.env, VITE_NATIVE_ENGINE_URL: target.url, VITE_NATIVE_ENGINE_TOKEN: target.token },
});

async function readTarget(stdout: ReadableStream<Uint8Array>) {
    const found: Record<string, string> = {};
    let buffered = "";
    for await (const chunk of stdout) {
        buffered += new TextDecoder().decode(chunk);
        for (const line of buffered.split("\n")) {
            const [key, value] = line.trim().split(" ");
            if (key === "url" || key === "token") found[key] = value;
        }
        if (found.url && found.token) return { url: found.url, token: found.token };
    }
    throw new Error("drift-engined exited before reporting its address");
}

async function shutdown() {
    engine.kill();
    vite.kill();
    await Promise.all([engine.exited, vite.exited]);
    rmSync(runtime, { recursive: true, force: true });
    process.exit(0);
}
process.on("SIGINT", () => void shutdown());
process.on("SIGTERM", () => void shutdown());
await Promise.race([engine.exited, vite.exited]);
await shutdown();
