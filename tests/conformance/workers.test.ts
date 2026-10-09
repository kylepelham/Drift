import { fakeAnthropic, model, sse, startEngine, type Engine, type Frame } from "./harness";
import { afterAll, beforeAll, expect, test } from "bun:test";
import path from "node:path";

const root = path.resolve(import.meta.dir, "../..");
let fake: ReturnType<typeof fakeAnthropic>;
let engine: Engine;

beforeAll(async () => {
    const build = Bun.spawnSync(["cargo", "build", "-q", "-p", "drift-engined"], {
        cwd: root,
        stdout: "inherit",
        stderr: "inherit",
    });
    if (build.exitCode !== 0) throw new Error("drift-engined did not build");
    fake = fakeAnthropic();
    engine = await startEngine(fake.url);
}, 300_000);

afterAll(async () => {
    await engine.stop();
    engine.cleanup();
    fake.stop();
}, 30_000);

type Task = {
    id: string;
    state: string;
    mode: string;
    delivered: boolean;
    held: boolean;
    description: string;
    result?: string;
};
const submit = (session: string, text: string) =>
    engine.call("POST", `/sessions/${session}/turns`, { parts: [{ type: "text", text }], model });
const taskFrame = (predicate: (task: Task) => boolean) => (frame: Frame) =>
    frame.type === "task.updated" && predicate(frame.task as Task);
const launch = (description: string, prompt: string) =>
    sse.toolUse("task", { description, prompt, run_in_background: true });

test("a background worker reports its progress, the parent carries on, and the result arrives once", async () => {
    const session = await engine.setup();
    const events = engine.events();
    await events.opened;
    fake.push(
        { match: "PARENT survey", body: launch("Survey", "CHILD survey the code") },
        { match: "PARENT survey", body: sse.text("working meanwhile") },
        { match: "PARENT survey", body: sse.text("thanks for the survey") },
        { match: "CHILD survey", body: sse.text("three things found"), delayMs: 1_500 },
    );
    await submit(session, "PARENT survey");
    const running = await events.until(taskFrame((task) => task.state === "running"));
    const id = (running.task as Task).id;
    await events.until((f) => f.type === "session.status" && f.sessionId === session && f.status === "idle");
    const during = await engine.call<Task[]>("GET", `/sessions/${session}/tasks`);
    expect(during.json[0]!.state).toBe("running");

    await events.until(
        taskFrame((task) => task.id === id && task.delivered),
        15_000,
    );
    const finished = await engine.call<Task>("GET", `/tasks/${id}`);
    expect([finished.json.state, finished.json.result]).toEqual(["replied", "three things found"]);
    const deliveredAt = events.frames.findIndex(taskFrame((task) => task.id === id && task.delivered));
    await events.until(
        (f) =>
            f.type === "session.status" &&
            f.sessionId === session &&
            f.status === "idle" &&
            events.frames.indexOf(f) > deliveredAt,
        15_000,
    );
    const messages = await engine.call<{ role: string; parts: { type: string; text?: string }[] }[]>(
        "GET",
        `/sessions/${session}/messages`,
    );
    const results = messages.json.flatMap((m) => m.parts).filter((p) => p.type === "task_result");
    expect(results.map((p) => p.text)).toEqual(["three things found"]);
    expect(messages.json.at(-1)!.parts[0]!.text).toBe("thanks for the survey");

    events.close();
    const replay = engine.events(0);
    await replay.opened;
    await replay.until(taskFrame((task) => task.id === id && task.delivered));
    replay.close();
}, 30_000);

test("session Stop ends a background worker while the parent is idle and wakes nothing", async () => {
    const session = await engine.setup();
    const events = engine.events();
    await events.opened;
    fake.push(
        { match: "PARENT stop", body: launch("Long job", "CHILD long job") },
        { match: "PARENT stop", body: sse.text("launched it") },
        { match: "CHILD long job", body: sse.text("too late"), delayMs: 5_000 },
    );
    await submit(session, "PARENT stop");
    const running = await events.until(
        taskFrame((task) => task.state === "running" && task.description === "Long job"),
    );
    await events.until((f) => f.type === "session.status" && f.sessionId === session && f.status === "idle");
    const requests = fake.seen.length;
    const stopped = await engine.call<{ aborted: boolean }>("POST", `/sessions/${session}/abort`);
    expect(stopped.json.aborted).toBe(true);
    // Held for the next prompt rather than marked handed over: nothing carried it yet.
    const ended = await events.until(
        taskFrame(
            (task) => task.id === (running.task as Task).id && task.state === "stopped" && task.held && !task.delivered,
        ),
    );
    expect((ended.task as Task).mode).toBe("background");
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(fake.seen.length).toBe(requests);
    const listed = await engine.call<Task[]>("GET", `/sessions/${session}/tasks`);
    expect(listed.json.map((t) => t.state)).toEqual(["stopped"]);
    events.close();
}, 30_000);
