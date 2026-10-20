import { fakeAnthropic, model, sse, startEngine, type Engine, type Frame } from "./harness";
import { afterAll, beforeAll, expect, test } from "bun:test";
import { existsSync } from "node:fs";
import path from "node:path";

const root = path.resolve(import.meta.dir, "../..");
const page =
    "data:text/html,<title>Pane</title><textarea id=note style='position:fixed;inset:0;width:100%;height:100%'></textarea>" +
    "<script>addEventListener('input', () => document.title = 'Typed ' + note.value)</script>";

/** Where Chrome or Edge is installed on Windows; elsewhere the test only runs where a browser is found the same way. */
function browserInstalled() {
    const roots = [process.env.ProgramFiles, process.env["ProgramFiles(x86)"], process.env.LOCALAPPDATA];
    const programs = [
        String.raw`Google\Chrome\Application\chrome.exe`,
        String.raw`Microsoft\Edge\Application\msedge.exe`,
    ];
    const apps = [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    ];

    const windows = roots.some(
        (folder) => folder && programs.some((program) => existsSync(path.join(folder, program))),
    );
    return windows || apps.some((app) => existsSync(app));
}

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
    // The browser's files stay locked for a moment after its job ends with the engine.
    for (let attempt = 0; ; attempt++) {
        try {
            engine.cleanup();
            break;
        } catch (cause) {
            if (attempt === 40) throw cause;
            await Bun.sleep(250);
        }
    }
    fake.stop();
}, 30_000);

const browserFrame = (state: string) => (frame: Frame) =>
    frame.type === "browser.updated" && (frame.browser as { state: string }).state === state;

/** Waits for the first binary message on a view socket that is a frame, and returns its header. */
function firstFrame(socket: WebSocket) {
    return new Promise<{ width: number; height: number }>((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error("no frame within 15 s")), 15_000);
        socket.onmessage = (message) => {
            if (!(message.data instanceof ArrayBuffer)) return;
            const length = new DataView(message.data).getUint32(0, true);
            const header = JSON.parse(new TextDecoder().decode(new Uint8Array(message.data, 4, length)));
            const jpeg = new Uint8Array(message.data, 4 + length, 2);
            expect([...jpeg]).toEqual([0xff, 0xd8]);
            clearTimeout(timer);
            resolve(header);
        };
    });
}

test.skipIf(!browserInstalled())(
    "a browser the agent opens streams to a view socket, takes the pane's typing, and ends on stop",
    async () => {
        const session = await engine.setup();
        const events = engine.events();
        await events.opened;
        fake.push({ body: sse.toolUse("browser", { action: "open", url: page }) }, { body: sse.text("Opened.") });

        await engine.call("POST", `/sessions/${session}/turns`, { parts: [{ type: "text", text: "open it" }], model });
        const started = await events.until(browserFrame("running"), 30_000);
        expect((started.browser as { sessionId: string }).sessionId).toBe(session);

        const record = await engine.call<{ pages: { url: string }[] }>("GET", `/sessions/${session}/browser`);
        expect(record.json.pages.length).toBeGreaterThan(0);

        const viewUrl = `${engine.url.replace(/^http/, "ws")}/sessions/${session}/browser/view?token=${engine.token}`;
        const socket = new WebSocket(viewUrl);
        socket.binaryType = "arraybuffer";
        const header = await firstFrame(socket);
        expect(header.width).toBeGreaterThan(0);

        // As a user does: click into the page, then type.
        const click = { type: "mouse", x: header.width / 2, y: header.height / 2, button: "left", clickCount: 1 };
        socket.send(JSON.stringify({ ...click, event: "down" }));
        socket.send(JSON.stringify({ ...click, event: "up" }));
        socket.send(JSON.stringify({ type: "text", text: "Ada" }));
        await events.until(
            (frame) =>
                frame.type === "browser.updated" &&
                (frame.browser as { pages: { title: string }[] }).pages.some((tab) => tab.title === "Typed Ada"),
            15_000,
        );
        socket.close();

        await engine.call("POST", `/sessions/${session}/browser/stop`);
        await events.until(browserFrame("stopped"), 30_000);
        const gone = await engine.call("GET", `/sessions/${session}/browser`);
        expect(gone.json).toBeNull();
        events.close();
    },
    90_000,
);
