import { shellInvoke, type ShellInvoke } from "../../shell";
// Where the native engine lives: the shell reports it once bound; the companion reaches it through the
// gateway, which signs the device in and adds the engine's token itself; browser dev reads env from drift-engined.
import { remoteEngineBase } from "../../runtime";

import type { Target } from "./client";

type ShellStatus = { url?: string; token?: string; error?: string };

const readyTimeoutMs = 15_000;
const pollMs = 100;

export async function resolveTarget(): Promise<Target> {
    const invoke = shellInvoke();
    if (invoke) return waitForShell(invoke);
    const gateway = remoteEngineBase();
    if (gateway) return { url: gateway, token: "" };
    const url = import.meta.env.VITE_NATIVE_ENGINE_URL;
    const token = import.meta.env.VITE_NATIVE_ENGINE_TOKEN;
    if (!url || !token)
        throw new Error("VITE_NATIVE_ENGINE_URL and VITE_NATIVE_ENGINE_TOKEN are required outside the shell");
    return { url, token };
}

async function waitForShell(invoke: ShellInvoke): Promise<Target> {
    const deadline = Date.now() + readyTimeoutMs;
    for (;;) {
        const status = await invoke<ShellStatus>("native_engine_status");
        if (status.url && status.token) return { url: status.url, token: status.token };
        if (status.error) throw new Error(status.error);
        if (Date.now() >= deadline) throw new Error("native engine did not bind in time");
        await new Promise((resolve) => setTimeout(resolve, pollMs));
    }
}
