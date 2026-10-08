// Engine benchmark: Drift 2's native engine against the opencode engine Drift 1.3 shipped, on the same
// stub provider (it answers at once, so only engine time is measured) and the same workloads.
// `bun run bench:engine [runs] [--legacy <1.3 install dir>]`. Results feed docs/engine-rewrite.md.
import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import os from "node:os";

const root = path.resolve(import.meta.dirname, "..");
const args = process.argv.slice(2);
const legacyAt = args.indexOf("--legacy");
const legacyDir = legacyAt >= 0 ? args[legacyAt + 1] : undefined;
const runs = Number(args.find((arg, i) => /^\d+$/.test(arg) && args[i - 1] !== "--legacy") ?? 3);
const WARM_TURNS = 50;
const SESSIONS = 200;
const READS = 20;

type Request = { at: number; marker?: string; lastRole: string; tools: unknown[]; system: number };

/** An OpenAI-compatible endpoint that answers at once: `turn N` with `done N`, and `read N` with a read of `file`. */
function stubProvider(file: string) {
    const requests: Request[] = [];
    const sent: { marker: string; at: number }[] = [];
    const chunk = (delta: object, finish: string | null = null) =>
        `data: ${JSON.stringify({ id: "b", object: "chat.completion.chunk", created: 0, model: "stub", choices: [{ index: 0, delta, finish_reason: finish }] })}\n\n`;
    const server = Bun.serve({
        port: 0,
        fetch: async (request) => {
            const at = performance.now();
            const body = (await request.json()) as {
                messages?: { role: string; content: unknown }[];
                tools?: { function?: { name: string; parameters?: { properties?: Record<string, unknown> } } }[];
            };
            const messages = body.messages ?? [];
            const users = messages.filter((m) => m.role === "user").map((m) => JSON.stringify(m.content));
            const marker = requestMarker(users);
            const last = messages.at(-1)?.role ?? "";
            const tools = body.tools ?? [];
            const system = messages
                .filter((m) => m.role === "system")
                .reduce((n, m) => n + JSON.stringify(m.content).length, 0);
            requests.push({ at, marker, lastRole: last, tools, system });
            const read = tools.find((tool) => tool.function?.name === "read");
            if (marker?.startsWith("read") && last === "user" && read) {
                const key =
                    Object.keys(read.function?.parameters?.properties ?? {}).find(
                        (name) => name === "filePath" || name === "path",
                    ) ?? "path";
                const call = {
                    index: 0,
                    id: `call_${marker.replace(" ", "_")}`,
                    type: "function",
                    function: { name: "read", arguments: JSON.stringify({ [key]: file }) },
                };
                const text =
                    chunk({ role: "assistant", tool_calls: [call] }) + chunk({}, "tool_calls") + "data: [DONE]\n\n";
                sent.push({ marker, at: performance.now() });
                return new Response(text, { headers: { "content-type": "text/event-stream" } });
            }
            const reply = marker ? `done ${marker.split(" ")[1]}` : "ok";
            const text =
                chunk({ role: "assistant", content: "" }) +
                chunk({ content: reply }) +
                chunk({}, "stop") +
                "data: [DONE]\n\n";
            return new Response(text, { headers: { "content-type": "text/event-stream" } });
        },
    });
    // Title requests carry no tools; only a turn's do.
    const turn = (marker: string, after = (r: Request) => r.lastRole === "user") =>
        requests.find((r) => r.tools.length > 0 && r.marker === marker && after(r));
    return { url: `http://127.0.0.1:${server.port}/v1`, requests, sent, turn, stop: () => server.stop(true) };
}

type Stub = ReturnType<typeof stubProvider>;

function requestMarker(users: string[]) {
    return users
        .at(-1)
        ?.match(/(turn|read) (\d+)/)
        ?.slice(1)
        .join(" ");
}

/** What a benchmark needs of an engine, whichever it is. */
type Engine = {
    pid: number;
    frames: { at: number; text: string }[];
    createSession(): Promise<string>;
    prompt(session: string, text: string): Promise<void>;
    listSessions(): Promise<void>;
    messages(session: string): Promise<void>;
    stop(): Promise<void>;
};

async function readUntil<T>(stdout: ReadableStream<Uint8Array>, matches: (text: string) => T | undefined) {
    let buffered = "";
    for await (const chunk of stdout) {
        buffered += new TextDecoder().decode(chunk);
        const found = matches(buffered);
        if (found !== undefined) return found;
    }
    throw new Error("process exited before reporting");
}

async function json(response: Response) {
    if (!response.ok) throw new Error(`${response.url} ${response.status}: ${await response.text()}`);
    const text = await response.text();
    return text ? JSON.parse(text) : undefined;
}

async function startNative(stub: Stub, dirs: Dirs): Promise<Engine> {
    mkdirSync(path.join(dirs.home, ".config", "drift"), { recursive: true });
    const providers = {
        bench: {
            name: "Bench",
            baseUrl: stub.url,
            apiKeyEnv: "DRIFT_BENCH_KEY",
            models: { stub: { context: 128000, output: 8192 } },
        },
    };
    writeFileSync(path.join(dirs.home, ".config", "drift", "drift.json"), JSON.stringify({ providers }));
    const proc = Bun.spawn([path.join(root, "target", "release", "drift-engined.exe"), "--data-dir", dirs.data], {
        stdout: "pipe",
        stderr: "ignore",
        env: { ...process.env, USERPROFILE: dirs.home, HOME: dirs.home, DRIFT_BENCH_KEY: "bench" },
    });
    const { url, token } = await readUntil(proc.stdout, (text) => {
        const url = text.match(/url (\S+)/)?.[1];
        const token = text.match(/token (\S+)/)?.[1];
        return url && token ? { url, token } : undefined;
    });
    const frames: Engine["frames"] = [];
    const socket = new WebSocket(`${url.replace(/^http/, "ws")}/events?token=${token}`);
    await new Promise<void>((resolve, reject) => {
        socket.onmessage = (message) => {
            frames.push({ at: performance.now(), text: String(message.data) });
            resolve();
        };
        socket.onerror = () => reject(new Error("event socket failed"));
    });
    const call = (method: string, route: string, body?: unknown) =>
        fetch(`${url}${route}`, {
            method,
            headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
            body: body === undefined ? undefined : JSON.stringify(body),
        }).then(json);
    const workspace = await call("POST", "/workspaces", { path: dirs.workspace, name: "bench" });
    return {
        pid: proc.pid,
        frames,
        createSession: async () =>
            (
                await call("POST", "/sessions", {
                    workspaceId: workspace.id,
                    title: "Bench",
                    model: { provider: "bench", model: "stub" },
                })
            ).id,
        prompt: async (session, text) =>
            void (await call("POST", `/sessions/${session}/turns`, { parts: [{ type: "text", text }] })),
        listSessions: async () => void (await call("GET", `/sessions?workspace=${workspace.id}&limit=${SESSIONS}`)),
        messages: async (session) => void (await call("GET", `/sessions/${session}/messages`)),
        stop: async () => {
            socket.close();
            proc.kill();
            await proc.exited;
        },
    };
}

/** Drift 1.3's engine as its shell started it: the bundled opencode with Drift's plugins, in throwaway data and config folders. */
async function startLegacy(stub: Stub, dirs: Dirs, install: string): Promise<Engine> {
    const extensions = path.join(install, "drift-extensions");
    const config = path.join(dirs.home, "runtime");
    mkdirSync(path.join(config, "pending"), { recursive: true });
    const base = await Bun.file(path.join(extensions, "opencode.json")).json();
    const plugin = (name: string) => path.join(extensions, "plugin", `${name}.js`);
    writeFileSync(
        path.join(config, "mcp-approvals.json"),
        JSON.stringify({ version: 3, generation: 0, decisions: [] }),
    );
    writeFileSync(path.join(config, "prompt-overrides.json"), JSON.stringify({ version: 1, families: {} }));
    const provider = {
        bench: {
            npm: "@ai-sdk/openai-compatible",
            name: "Bench",
            options: { baseURL: stub.url, apiKey: "bench" },
            models: { stub: { name: "stub", tools: true, limit: { context: 128000, output: 8192 } } },
        },
    };
    writeFileSync(
        path.join(config, "opencode.json"),
        JSON.stringify({
            ...base,
            provider: { ...(base.provider ?? {}), ...provider },
            plugin: [
                ...base.plugin,
                plugin("spawn-thread"),
                [
                    plugin("prompt-overrides"),
                    {
                        catalogPath: path.join(extensions, "prompt-catalog.json"),
                        settingsPath: path.join(config, "prompt-overrides.json"),
                    },
                ],
                [
                    plugin("mcp-approval"),
                    {
                        policyPath: path.join(config, "mcp-approvals.json"),
                        pendingDirectory: path.join(config, "pending"),
                        sentinelPath: path.join(config, "mcp-fail-closed.json"),
                        generation: 0,
                    },
                ],
            ],
        }),
    );
    const xdg = (name: string) => path.join(dirs.data, name);
    const password = "bench";
    const proc = Bun.spawn(
        [path.join(install, "drift-engine.exe"), "serve", "--hostname", "127.0.0.1", "--port", "0"],
        {
            cwd: dirs.workspace,
            stdout: "pipe",
            stderr: "ignore",
            // Its data, config and state go to throwaway folders; the package cache is the user's, as 1.3 used it.
            env: {
                ...process.env,
                OPENCODE_CONFIG_DIR: config,
                OPENCODE_SERVER_PASSWORD: password,
                OPENCODE_SERVER_USERNAME: "opencode",
                DRIFT_MCP_APPROVAL_REQUIRED: "1",
                XDG_DATA_HOME: xdg("data"),
                XDG_CONFIG_HOME: xdg("config"),
                XDG_STATE_HOME: xdg("state"),
            },
        },
    );
    const url = await readUntil(proc.stdout, (text) => text.match(/listening on (http\S+)/)?.[1]);
    const headers = {
        authorization: `Basic ${btoa(`opencode:${password}`)}`,
        "x-opencode-directory": encodeURIComponent(dirs.workspace),
        "content-type": "application/json",
    };
    const frames: Engine["frames"] = [];
    const events = await fetch(`${url}/global/event`, { headers });
    const reader = events.body!.getReader();
    let connected!: () => void;
    const ready = new Promise<void>((resolve) => (connected = resolve));
    void (async () => {
        const decoder = new TextDecoder();
        let buffered = "";
        for (;;) {
            const { value, done } = await reader.read().catch(() => ({ value: undefined, done: true }));
            if (done) return;
            buffered += decoder.decode(value, { stream: true });
            const lines = buffered.split("\n");
            buffered = lines.pop() ?? "";
            for (const line of lines.filter((l) => l.startsWith("data: "))) {
                frames.push({ at: performance.now(), text: line });
                if (line.includes("server.connected")) connected();
            }
        }
    })();
    await ready;
    const call = (method: string, route: string, body?: unknown) =>
        fetch(`${url}${route}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) }).then(
            json,
        );
    return {
        pid: proc.pid,
        frames,
        createSession: async () => (await call("POST", "/session", {})).id,
        prompt: async (session, text) =>
            void (await call("POST", `/session/${session}/prompt_async`, {
                parts: [{ type: "text", text }],
                model: { providerID: "bench", modelID: "stub" },
            })),
        listSessions: async () => void (await call("GET", "/session")),
        messages: async (session) => void (await call("GET", `/session/${session}/message`)),
        stop: async () => {
            await reader.cancel().catch(() => {});
            proc.kill();
            await proc.exited;
        },
    };
}

type Dirs = { home: string; data: string; workspace: string };

const median = (values: number[]) => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)];

async function until<T>(what: string, find: () => T | undefined, limit = 60_000): Promise<T> {
    const deadline = performance.now() + limit;
    for (;;) {
        const found = find();
        if (found !== undefined) return found;
        if (performance.now() > deadline) throw new Error(`timed out waiting for ${what}`);
        await Bun.sleep(1);
    }
}

/** Working set now and at peak, and processor time used so far, as Windows counts them. */
function processStats(pid: number) {
    const script = `$p = Get-Process -Id ${pid}; "$($p.WorkingSet64) $($p.PeakWorkingSet64) $($p.TotalProcessorTime.TotalMilliseconds)"`;
    const [ws, peak, cpu] = Bun.spawnSync(["powershell", "-NoProfile", "-Command", script])
        .stdout.toString()
        .trim()
        .split(" ")
        .map(Number);
    return { ws, peak, cpu };
}

async function timeEach(times: number, act: () => Promise<void>) {
    const taken: number[] = [];
    for (let i = 0; i < times; i += 1) {
        const started = performance.now();
        await act();
        taken.push(performance.now() - started);
    }
    return median(taken);
}

/** One run: start the engine, then every workload on it. */
async function run(start: (stub: Stub, dirs: Dirs) => Promise<Engine>) {
    const dirs = {
        home: mkdtempSync(path.join(os.tmpdir(), "drift-bench-home-")),
        data: mkdtempSync(path.join(os.tmpdir(), "drift-bench-data-")),
        workspace: mkdtempSync(path.join(os.tmpdir(), "drift-bench-ws-")),
    };
    const file = path.join(dirs.workspace, "notes.md");
    writeFileSync(file, "a line\n".repeat(200));
    const stub = stubProvider(file);
    const started = performance.now();
    const engine = await start(stub, dirs);
    const out: Record<string, number> = { coldStartMs: performance.now() - started };
    try {
        const session = await engine.createSession();
        const idleAfter = (since: number) =>
            until(
                "idle",
                () =>
                    engine.frames.find(
                        (f) =>
                            f.at > since &&
                            f.text.includes(session) &&
                            (f.text.includes("session.idle") || /"session\.status"[^]*"idle"/.test(f.text)),
                    )?.at,
            );
        const turn = async (n: number) => {
            const promptAt = performance.now();
            await engine.prompt(session, `turn ${n}`);
            const request = await until(`request ${n}`, () => stub.turn(`turn ${n}`));
            const textAt = await until(
                `text ${n}`,
                () => engine.frames.find((f) => f.at > request.at && f.text.includes(`done ${n}`))?.at,
            );
            const idleAt = await idleAfter(textAt);
            return {
                toProvider: request.at - promptAt,
                toEvent: textAt - request.at,
                total: idleAt - promptAt,
                request,
            };
        };
        const first = await turn(0);
        out.firstPromptToProviderMs = first.toProvider;
        out.systemChars = first.request.system;
        out.toolsChars = JSON.stringify(first.request.tools).length;
        const warm = [];
        for (let n = 1; n <= WARM_TURNS; n += 1) warm.push(await turn(n));
        out.warmPromptToProviderMs = median(warm.map((t) => t.toProvider));
        out.providerToEventMs = median(warm.map((t) => t.toEvent));
        out.turnMs = median(warm.map((t) => t.total));
        const reads = [];
        for (let n = 0; n < 5; n += 1) {
            await engine.prompt(session, `read ${n}`);
            const after = await until(`tool result ${n}`, () => stub.turn(`read ${n}`, (r) => r.lastRole === "tool"));
            const toolSent = stub.sent.find((s) => s.marker === `read ${n}`)!.at;
            reads.push(after.at - toolSent);
            await idleAfter(after.at);
        }
        out.toolRoundTripMs = median(reads);
        const busy = processStats(engine.pid);
        await Bun.sleep(10_000);
        const idle = processStats(engine.pid);
        out.idleCpuMsPer10s = idle.cpu - busy.cpu;
        out.workingSetMB = idle.ws / 2 ** 20;
        out.peakWorkingSetMB = idle.peak / 2 ** 20;
        out.sessionCreateMs = await timeEach(SESSIONS - 1, async () => void (await engine.createSession()));
        out.listSessionsMs = await timeEach(READS, () => engine.listSessions());
        out.loadHistoryMs = await timeEach(READS, () => engine.messages(session));
    } finally {
        await engine.stop();
        stub.stop();
        for (const dir of Object.values(dirs)) rmSync(dir, { recursive: true, force: true });
    }
    return out;
}

/** Every file under `dir`, in megabytes. */
function sizeMB(dir: string): number {
    return readdirSync(dir, { withFileTypes: true }).reduce((n, entry) => {
        const full = path.join(dir, entry.name);
        return n + (entry.isDirectory() ? sizeMB(full) : statSync(full).size / 2 ** 20);
    }, 0);
}

async function measure(name: string, start: (stub: Stub, dirs: Dirs) => Promise<Engine>) {
    const results: Record<string, number>[] = [];
    for (let i = 0; i < runs; i += 1) {
        results.push(await run(start));
        console.error(`${name}: run ${i + 1} of ${runs} done`);
    }
    const keys = Object.keys(results[0]);
    return Object.fromEntries(keys.map((key) => [key, median(results.map((r) => r[key]))]));
}

Bun.spawnSync(["cargo", "build", "-q", "--release", "-p", "drift-engined"], {
    cwd: root,
    stdout: "inherit",
    stderr: "inherit",
});
const native = await measure("native", startNative);
native.engineBinaryMB = statSync(path.join(root, "target", "release", "drift.exe")).size / 2 ** 20;
const table: Record<string, Record<string, number>> = { native };
if (legacyDir) {
    if (!existsSync(path.join(legacyDir, "drift-engine.exe"))) throw new Error(`no drift-engine.exe in ${legacyDir}`);
    const legacy = await measure("1.3", (stub, dirs) => startLegacy(stub, dirs, legacyDir));
    legacy.engineBinaryMB =
        statSync(path.join(legacyDir, "drift.exe")).size / 2 ** 20 +
        statSync(path.join(legacyDir, "drift-engine.exe")).size / 2 ** 20 +
        sizeMB(path.join(legacyDir, "drift-extensions"));
    table.legacy = legacy;
}
const round = (n: number) => (n >= 100 ? Math.round(n) : Math.round(n * 10) / 10);
const rows = Object.keys(native).map((key) => ({
    measure: key,
    ...(table.legacy ? { "1.3": round(table.legacy[key]) } : {}),
    "2.0": round(native[key]),
}));
console.log(JSON.stringify({ runs, warmTurns: WARM_TURNS, sessions: SESSIONS, rows }, null, 2));
