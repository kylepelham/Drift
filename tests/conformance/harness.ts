// Drives a real drift-engined over HTTP and WS against a fake Anthropic that replays recorded streams.
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { randomBytes } from "node:crypto";
import path from "node:path";
import os from "node:os";

const root = path.resolve(import.meta.dir, "../..");
const binary = path.join(root, "target", "debug", process.platform === "win32" ? "drift-engined.exe" : "drift-engined");
export const model = { provider: "anthropic", model: "claude-sonnet-4-5" };

/** `match` sends the reply only to the conversation whose first message contains it, so sessions running at once get their own. */
export type Scripted = { status?: number; body: string; delayMs?: number; match?: string };
type Seen = { headers: Record<string, string>; body: Record<string, unknown> };

export function fixture(name: string) {
    return readFileSync(path.join(import.meta.dir, "fixtures", `${name}.sse`), "utf8");
}

const event = (type: string, data: Record<string, unknown>) =>
    `event: ${type}\ndata: ${JSON.stringify({ type, ...data })}\n\n`;
const opening = event("message_start", {
    message: { id: "msg_s", role: "assistant", content: [], usage: { input_tokens: 5, output_tokens: 1 } },
});
const closing = (stop: string) =>
    event("message_delta", { delta: { stop_reason: stop }, usage: { output_tokens: 3 } }) + event("message_stop", {});

/** Streams built on the spot: a plain reply, and one tool call. */
export const sse = {
    text: (text: string) =>
        opening +
        event("content_block_start", { index: 0, content_block: { type: "text", text: "" } }) +
        event("content_block_delta", { index: 0, delta: { type: "text_delta", text } }) +
        event("content_block_stop", { index: 0 }) +
        closing("end_turn"),
    toolUse: (name: string, input: Record<string, unknown>) =>
        opening +
        event("content_block_start", {
            index: 0,
            content_block: { type: "tool_use", id: `toolu_${name}`, name, input: {} },
        }) +
        event("content_block_delta", {
            index: 0,
            delta: { type: "input_json_delta", partial_json: JSON.stringify(input) },
        }) +
        event("content_block_stop", { index: 0 }) +
        closing("tool_use"),
};

function firstMessageText(body: Record<string, unknown>) {
    const content = (body.messages as { content: string | { text?: string }[] }[] | undefined)?.[0]?.content;
    return typeof content === "string" ? content : (content ?? []).map((block) => block.text ?? "").join("");
}

/** A stand-in api.anthropic.com: answers `/v1/messages` from a queue and remembers what it was sent. */
export function fakeAnthropic() {
    const queue: Scripted[] = [];
    const seen: Seen[] = [];
    const take = (body: Record<string, unknown>) => {
        const first = firstMessageText(body);
        const keyed = queue.findIndex((reply) => reply.match !== undefined && first.includes(reply.match));
        const index = keyed >= 0 ? keyed : queue.findIndex((reply) => reply.match === undefined);
        return index >= 0 ? queue.splice(index, 1)[0] : undefined;
    };
    const server = Bun.serve({
        port: 0,
        fetch: async (request) => {
            if (new URL(request.url).pathname !== "/v1/messages") return new Response("not found", { status: 404 });
            const body = (await request.json()) as Record<string, unknown>;
            seen.push({ headers: Object.fromEntries(request.headers.entries()), body });
            const next = take(body) ?? {
                status: 500,
                body: JSON.stringify({ error: { type: "api_error", message: "fake ran out of responses" } }),
            };
            if (next.delayMs) await new Promise((resolve) => setTimeout(resolve, next.delayMs));
            const status = next.status ?? 200;
            const type = status === 200 ? "text/event-stream" : "application/json";
            // Small chunks force frame boundaries to land mid-line, like a real network.
            const chunks = next.body.match(/[\s\S]{1,13}/g) ?? [];
            const stream = new ReadableStream<Uint8Array>({
                async pull(controller) {
                    const chunk = chunks.shift();
                    if (chunk === undefined) return controller.close();
                    controller.enqueue(new TextEncoder().encode(chunk));
                },
            });
            return new Response(status === 200 ? stream : next.body, { status, headers: { "content-type": type } });
        },
    });
    return {
        url: `http://127.0.0.1:${server.port}`,
        push: (...responses: Scripted[]) => queue.push(...responses),
        seen,
        stop: () => server.stop(true),
    };
}

export type Engine = Awaited<ReturnType<typeof startEngine>>;
const credentialKeys = new Map<string, string>();

export async function startEngine(
    providerUrl: string,
    dataDir = mkdtempSync(path.join(os.tmpdir(), "drift-conformance-")),
) {
    const credentialKey = credentialKeys.get(dataDir) ?? randomBytes(32).toString("base64");
    credentialKeys.set(dataDir, credentialKey);
    const proc = Bun.spawn([binary, "--data-dir", dataDir, "--file-credentials"], {
        stdout: "pipe",
        stderr: "inherit",
        env: { ...process.env, DRIFT_ANTHROPIC_BASE_URL: providerUrl, DRIFT_CREDENTIALS_KEY: credentialKey },
    });
    const { url, token } = await readTarget(proc.stdout);
    const headers = { authorization: `Bearer ${token}`, "content-type": "application/json" };
    const call = async <T>(method: string, route: string, body?: unknown): Promise<{ status: number; json: T }> => {
        const response = await fetch(`${url}${route}`, {
            method,
            headers,
            body: body === undefined ? undefined : JSON.stringify(body),
        });
        const json = (await response.json().catch(() => null)) as T;
        return { status: response.status, json };
    };
    const workspace = mkdtempSync(path.join(os.tmpdir(), "drift-conformance-ws-"));
    return {
        url,
        token,
        dataDir,
        workspace,
        call,
        async setup() {
            await call("PUT", "/providers/anthropic/key", { key: "sk-conformance" });
            const ws = await call<{ id: string }>("POST", "/workspaces", { path: workspace, name: "ws" });
            // Titled, so the background title request stays out of the recorded exchanges.
            const session = await call<{ id: string }>("POST", "/sessions", {
                workspaceId: ws.json.id,
                model,
                title: "Conformance",
            });
            return session.json.id;
        },
        events(cursor?: number) {
            return openEvents(url, token, cursor);
        },
        stop() {
            proc.kill();
            return proc.exited;
        },
        cleanup() {
            credentialKeys.delete(dataDir);
            rmSync(dataDir, { recursive: true, force: true });
            rmSync(workspace, { recursive: true, force: true });
        },
    };
}

async function readTarget(stdout: ReadableStream<Uint8Array>) {
    let buffered = "";
    for await (const chunk of stdout) {
        buffered += new TextDecoder().decode(chunk);
        const url = buffered.match(/url (\S+)/)?.[1];
        const token = buffered.match(/token (\S+)/)?.[1];
        if (url && token) return { url, token };
    }
    throw new Error("drift-engined exited before reporting its address");
}

export type Frame = Record<string, unknown> & { type: string; seq?: number };

/** A WS client that queues frames so a test can wait for the one it cares about. */
function openEvents(url: string, token: string, cursor?: number) {
    const frames: Frame[] = [];
    const waiters: ((frame: Frame) => void)[] = [];
    const socket = new WebSocket(
        `${url.replace(/^http/, "ws")}/events?token=${token}${cursor === undefined ? "" : `&cursor=${cursor}`}`,
    );
    socket.onmessage = (message) => {
        const frame = JSON.parse(String(message.data)) as Frame;
        frames.push(frame);
        for (const waiter of waiters.splice(0)) waiter(frame);
    };
    const opened = new Promise<void>((resolve, reject) => {
        socket.onopen = () => resolve();
        socket.onerror = () => reject(new Error("socket failed"));
    });
    return {
        opened,
        frames,
        send: (frame: unknown) => socket.send(JSON.stringify(frame)),
        close: () => socket.close(),
        async until(predicate: (frame: Frame) => boolean, timeoutMs = 10_000): Promise<Frame> {
            const found = frames.find(predicate);
            if (found) return found;
            return new Promise((resolve, reject) => {
                const timer = setTimeout(
                    () =>
                        reject(
                            new Error(
                                `no frame matched within ${timeoutMs}ms; saw ${frames.map((f) => f.type).join(",")}`,
                            ),
                        ),
                    timeoutMs,
                );
                const check = (frame: Frame) => {
                    if (predicate(frame)) {
                        clearTimeout(timer);
                        resolve(frame);
                    } else waiters.push(check);
                };
                waiters.push(check);
            });
        },
    };
}
