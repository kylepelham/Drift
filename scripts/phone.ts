/**
 * Installs the latest Android build on a phone over wireless debugging, pairing it first by QR code when it has
 * never been paired with this PC. `bun run phone` (`--build` builds the APK first). See `docs/android.md`.
 *
 * Finding the phone needs mDNS, which `adb`'s own discovery sends only out of the lowest-metric adapter: with a VPN
 * or Hyper-V switch installed that is not the LAN, so this asks on every adapter itself.
 */
import { spawnSync } from "node:child_process";
import { randomInt } from "node:crypto";
import { writeFileSync } from "node:fs";
import { tempFolder } from "./temp";
import { renderSVG } from "uqr";
import dgram from "node:dgram";
import path from "node:path";
import os from "node:os";

const root = path.resolve(import.meta.dir, "..");
const apk = path.join(root, "src-android/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk");
const PAIRING = "_adb-tls-pairing._tcp.local";
const CONNECT = "_adb-tls-connect._tcp.local";

type Service = { instance: string; type: string; host: string; port: number };

function adb(...args: string[]) {
    const run = spawnSync("adb", args, { encoding: "utf8" });
    return { ok: run.status === 0, output: `${run.stdout}${run.stderr}`.trim() };
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

/** The adb services a response names, each with the address it came from when it names no A record. */
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
        const type = [PAIRING, CONNECT].find((service) => name.endsWith(`.${service}`));
        if (!type) return [];
        const instance = name.slice(0, -type.length - 1);
        return [{ instance, type, host: addresses.get(target.host) ?? from, port: target.port }];
    });
}

/** Asks every adapter's network for adb services until `wanted` accepts one, or `seconds` pass. */
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
                socket.send(query([PAIRING, CONNECT]), 5353, "224.0.0.251");
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

    const pairing = await discover((service) => service.type === PAIRING && service.instance === name, 300);
    if (!pairing) throw new Error("no phone scanned the code within five minutes");
    const paired = adb("pair", `${pairing.host}:${pairing.port}`, code);
    if (!paired.ok) throw new Error(`pairing failed: ${paired.output}`);
    console.log(paired.output);

    return pairing.host;
}

/** Connects to a paired phone that offers wireless debugging, the one at `host` when given. */
async function connect(host?: string, seconds = 8) {
    const offered = await discover((service) => service.type === CONNECT && (!host || service.host === host), seconds);
    if (!offered) return undefined;
    const serial = `${offered.host}:${offered.port}`;
    const connected = adb("connect", serial);

    return connected.ok && !connected.output.includes("failed") ? serial : undefined;
}

if (process.argv.includes("--build")) {
    const built = spawnSync("bunx", ["tauri", "android", "build", "--debug", "--apk", "--target", "aarch64"], {
        cwd: path.join(root, "src-android"),
        stdio: "inherit",
        shell: true,
    });
    if (built.status !== 0) process.exit(built.status ?? 1);
}

adb("start-server");
const serial = (await connect()) ?? (await connect(await pair(), 60));
if (!serial) throw new Error("paired, but the phone offered no connection; is Wireless debugging still on?");

const installed = adb("-s", serial, "install", "-r", apk);
console.log(installed.output);
if (!installed.ok) process.exit(1);
adb("-s", serial, "shell", "monkey", "-p", "dev.drift.remote", "1");
console.log(`Installed and opened on ${serial}.`);
