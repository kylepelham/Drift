/** The Drift app's own requests to its local proxy; the PCs themselves are reached only through it. */

import type { Connector, NearbyComputer, WaitingComputer } from "../state/computers";

export type Computer = {
    id: string;
    name: string;
    /** Its address on the network it was last reached on. */
    address?: string;
    /** Whether it can be reached away from its network, through its tunnel. */
    anywhere: boolean;
    active: boolean;
};

async function call<T>(method: "GET" | "POST", path: string, body?: unknown): Promise<T> {
    const response = await fetch(`/app/api${path}`, {
        method,
        credentials: "same-origin",
        headers: body === undefined ? undefined : { "content-type": "application/json" },
        body: body === undefined ? undefined : JSON.stringify(body),
    });
    const value = (await response.json().catch(() => null)) as (T & { error?: string }) | null;
    if (!response.ok || !value) throw new Error(value?.error || `request failed (${response.status})`);

    return value;
}

export type Computers = {
    computers: Computer[];
    /** This device's own Drift, on a desktop: its name. */
    local?: string | null;
};

export const appApi = {
    computers: () => call<Computers>("GET", "/computers"),
    use: (id: string) => call<{ ok: true }>("POST", "/use", { id }),
    forget: (id: string) => call<{ ok: true }>("POST", "/forget", { id }),
    /** Shows this device's own Drift again, on a desktop. */
    local: () => call<{ ok: true }>("POST", "/local"),
};

/** Finding computers and asking one to connect, through the proxy. */
export const appConnector: Connector = {
    nearby: () => call<{ nearby: NearbyComputer[] }>("GET", "/nearby").then((value) => value.nearby),
    ask: (address) => call<WaitingComputer>("POST", "/ask", { address }),
    answer: (id) => call<{ id: string }>("POST", "/answer", { id }),
    cancel: (id) => call("POST", "/cancel", { id }),
};

/** Opens Drift on the PC just chosen or linked. */
export function openDrift() {
    window.location.replace("/app");
}
