// Installs the Android app on a phone over wireless debugging, pairing it by QR code first if needed (docs/android.md).
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { driftTemp, tempFolder } from "./temp";
import { randomInt } from "node:crypto";
import { renderSVG } from "uqr";
import dgram from "node:dgram";
import path from "node:path";
import net from "node:net";
import os from "node:os";

const root = path.resolve(import.meta.dir, "..");
const apk = path.join(root, "src-android/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk");
const PAIRING = "_adb-tls-pairing._tcp.local";
/** The phone last installed on, so the next run finds it without pairing again. */
const remembered = path.join(driftTemp("phone"), "last-phone.txt");

type Service = { instance: string; host: string; port: number };

/** Runs adb, stopped after `seconds` by our own timer: spawnSync's fires at once after much network work in Bun. */
async function adb(args: string[], seconds = 8) {
    const run = Bun.spawn(["adb", ...args], { stdout: "pipe", stderr: "pipe" });
    const limit = setTimeout(() => run.kill(), seconds * 1000);
    const [stdout, stderr, status] = await Promise.all([
        new Response(run.stdout).text(),
        new Response(run.stderr).text(),
        run.exited,
    ]);
    clearTimeout(limit);

    return { ok: status === 0, output: `${stdout}${stderr}`.trim() };
}

function encodeName(name: string) {
    const labels = name.split(".").flatMap((label) => [Buffer.from([label.length]), Buffer.from(label)]);
    return Buffer.concat([...labels, Buffer.from([0])]);
}

/** A PTR question for each service type. */
function query(types: string[]) {
    const header = Buffer.from([0, 0, 0, 0, 0, types.length, 0, 0, 0, 0, 0, 0]);
    return Buffer.concat([
        header,
        ...types.map((type) => Buffer.concat([encodeName(type), Buffer.from([0, 12, 0, 1])])),
    ]);
}

function readName(packet: Buffer, start: number): [string, number] {
    const labels: string[] = [];
    let offset = start;
    let end = -1;
    for (let hops = 0; hops < 128; hops++) {
        const length = packet[offset]!;
        if (length === 0) return [labels.join("."), end < 0 ? offset + 1 : end];
        if ((length & 0xc0) === 0xc0) {
            if (end < 0) end = offset + 2;
            offset = ((length & 0x3f) << 8) | packet[offset + 1]!;
        } else {
            labels.push(packet.toString("utf8", offset + 1, offset + 1 + length));
            offset += 1 + length;
        }
    }
    throw new Error("name loops");
}

/** The pairing services a response names, each with the address it came from when it names no A record. */
function services(packet: Buffer, from: string): Service[] {
    const counts = [4, 6, 8, 10].map((at) => packet.readUInt16BE(at));
    let offset = 12;
    for (let index = 0; index < counts[0]!; index++) offset = readName(packet, offset)[1] + 4;
    const targets = new Map<string, { host: string; port: number }>();
    const addresses = new Map<string, string>();
    for (let index = 0; index < counts[1]! + counts[2]! + counts[3]!; index++) {
        const [name, next] = readName(packet, offset);
        const type = packet.readUInt16BE(next);
        const at = next + 10;
        if (type === 33) targets.set(name, { host: readName(packet, at + 6)[0], port: packet.readUInt16BE(at + 4) });
        if (type === 1) addresses.set(name, [...packet.subarray(at, at + 4)].join("."));
        offset = at + packet.readUInt16BE(next + 8);
    }

    return [...targets].flatMap(([name, target]) => {
        if (!name.endsWith(`.${PAIRING}`)) return [];
        const instance = name.slice(0, -PAIRING.length - 1);
        return [{ instance, host: addresses.get(target.host) ?? from, port: target.port }];
    });
}

/** Asks every adapter's network for phones waiting to pair until `wanted` accepts one, or `seconds` pass. */
function discover(wanted: (service: Service) => boolean, seconds: number) {
    const lan = Object.values(os.networkInterfaces())
        .flat()
        .filter((entry) => entry?.family === "IPv4" && !entry.internal)
        .map((entry) => entry!.address);
    const socket = dgram.createSocket({ type: "udp4", reuseAddr: true });

    return new Promise<Service | undefined>((resolve) => {
        const done = (service?: Service) => {
            clearInterval(ask);
            clearTimeout(limit);
            socket.close();
            resolve(service);
        };
        const ask = setInterval(() => send(), 1500);
        const limit = setTimeout(() => done(), seconds * 1000);
        const send = () => {
            for (const address of lan) {
                socket.setMulticastInterface(address);
                socket.send(query([PAIRING]), 5353, "224.0.0.251");
            }
        };
        socket.on("message", (packet, from) => {
            const found = (() => {
                try {
                    return services(packet, from.address).find(wanted);
                } catch {
                    return undefined;
                }
            })();
            if (found) done(found);
        });
        socket.bind(5353, () => {
            for (const address of lan) {
                try {
                    socket.addMembership("224.0.0.251", address);
                } catch {
                    // An adapter that cannot join, such as one that is down, is skipped.
                }
            }
            send();
        });
    });
}

/** Shows the pairing QR code, in the terminal and as a page, and pairs with the phone that scans it. */
async function pair() {
    const name = `drift-${randomInt(16 ** 6)
        .toString(16)
        .padStart(6, "0")}`;
    const code = String(randomInt(1_000_000)).padStart(6, "0");
    const text = `WIFI:T:ADB;S:${name};P:${code};;`;
    const page = path.join(tempFolder("phone", "pair-"), "pair.html");
    writeFileSync(
        page,
        `<!doctype html><title>Pair a phone with Drift</title><body style="margin:0;display:grid;place-items:center;height:100vh;background:#fff;font:16px system-ui"><div style="text-align:center;width:360px">${renderSVG(text)}<p>On the phone: Developer options, Wireless debugging, Pair device with QR code.</p></div></body>`,
    );
    console.log(`Scan the code on the phone (Wireless debugging, Pair device with QR code). Also at:\n${page}\n`);

    const pairing = await discover((service) => service.instance === name, 300);
    if (!pairing) throw new Error("no phone scanned the code within five minutes");
    const paired = await adb(["pair", `${pairing.host}:${pairing.port}`, code]);
    if (!paired.ok) throw new Error(`pairing failed: ${paired.output}`);
    console.log(paired.output);

    return pairing.host;
}

/** A phone adb already has online over the network. */
async function connected() {
    return (await adb(["devices"])).output
        .split("\n")
        .map((line) => line.trim().split(/\s+/))
        .find(([serial, state]) => serial?.includes(":") && state === "device")?.[0];
}

/** `adb connect`, keeping the serial only when the phone came online; a wrong port is dropped again. */
async function attach(serial: string) {
    if (!(await adb(["connect", serial])).output.startsWith("connected")) return undefined;
    // The phone shows as offline for a moment while it accepts this PC.
    for (let tries = 0; tries < 10; tries++) {
        if ((await connected()) === serial) return serial;
        await Bun.sleep(300);
    }
    await adb(["disconnect", serial]);
    return undefined;
}

function listening(host: string, port: number) {
    return new Promise<boolean>((resolve) => {
        const socket = net.connect({ host, port });
        const finish = (open: boolean) => {
            socket.resetAndDestroy();
            resolve(open);
        };
        socket.setTimeout(400, () => finish(false));
        socket.once("connect", () => finish(true));
        socket.once("error", () => finish(false));
    });
}

/** The phone's wireless debugging port, tried directly: some phones answer mDNS only while pairing. */
async function scan(host: string) {
    const open: number[] = [];
    let next = 32768;
    const worker = async () => {
        while (next <= 60999) {
            const port = next++;
            if (await listening(host, port)) open.push(port);
        }
    };
    await Promise.all(Array.from({ length: 800 }, worker));

    for (const port of open.sort((a, b) => a - b)) {
        const serial = await attach(`${host}:${port}`);
        if (serial) return serial;
    }
    return undefined;
}

/** Connects to a paired phone at `host` by trying its ports, since phones rarely answer mDNS for them. */
async function connect(host?: string) {
    if (!host) return undefined;
    console.log(`Looking for ${host}'s wireless debugging port...`);

    return scan(host);
}

function remember(serial: string) {
    writeFileSync(remembered, serial.split(":")[0]!);
}

const lastHost = () => (existsSync(remembered) ? readFileSync(remembered, "utf8").trim() || undefined : undefined);

if (process.argv.includes("--build")) {
    const built = spawnSync("bunx", ["tauri", "android", "build", "--debug", "--apk", "--target", "aarch64"], {
        cwd: path.join(root, "src-android"),
        stdio: "inherit",
        shell: true,
    });
    if (built.status !== 0) process.exit(built.status ?? 1);
}

await adb(["start-server"]);
const serial = (await connected()) ?? (await connect(lastHost())) ?? (await connect(await pair()));
if (!serial) {
    console.error("Paired, but the phone accepted no connection; is Wireless debugging still on?");
    process.exit(1);
}
remember(serial);
console.log(`Connected to ${serial}.`);

const install = () => adb(["-s", serial, "install", "-r", apk], 300);
let installed = await install();
console.log(installed.output);
// Another key signed the installed app: only removing it, and its data, lets this one in.
if (installed.output.includes("INSTALL_FAILED_UPDATE_INCOMPATIBLE")) {
    if (!process.argv.includes("--replace")) {
        console.error(
            "The app on the phone was signed with another key. Run again with --replace to uninstall it first;",
        );
        console.error("its data goes with it, so the PC is linked again in the app.");
        process.exit(1);
    }
    console.log((await adb(["-s", serial, "uninstall", "dev.drift.remote"])).output);
    installed = await install();
    console.log(installed.output);
}
if (!installed.ok) process.exit(1);
await adb(["-s", serial, "shell", "monkey", "-p", "dev.drift.remote", "1"]);
console.log(`Installed and opened on ${serial}.`);
process.exit(0);
