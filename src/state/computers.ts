/** Switching the desktop's main window between this PC's Drift and the Drift on a linked computer. On this PC the
 * shell does it (src-tauri/src/computers.rs); on another computer, shown through the shell's local proxy, the proxy's
 * own API does, and reports which PC it can return to. A phone's proxy has no PC of its own, so it offers no switcher. */

import { appConnector } from "../app/api";
import { isAppRuntime } from "../runtime";
import { shellInvoke } from "../shell";

export type LinkedComputer = {
    id: string;
    name: string;
    /** Its address on the network it was last reached on. */
    address?: string;
    /** Whether it can be reached away from its network, through its tunnel. */
    anywhere: boolean;
};

/** A computer with Remote Access on that answered on this network. */
export type NearbyComputer = { id: string; name: string; address: string; linked: boolean };

/** A computer asked to accept this device: the request waited on, and the code both screens show. */
export type WaitingComputer = { id: string; name: string; code: string };

/** Finding computers and asking one to accept this device; its user accepts or declines it. */
export type Connector = {
    nearby(): Promise<NearbyComputer[]>;
    ask(address: string): Promise<WaitingComputer>;
    /** Resolves once the computer accepted, to its id; rejects when it declined or the request ran out. */
    answer(request: string): Promise<{ id: string }>;
    cancel(request: string): Promise<unknown>;
};

export type ComputerSwitcher = {
    /** The linked computer this window shows; undefined while it shows this PC. */
    current?: string;
    list(): Promise<LinkedComputer[]>;
    /** Shows a linked computer's Drift in this window. */
    open(id: string): Promise<void>;
    /** Shows this PC's own Drift again. */
    home(): Promise<void>;
    /** Finds computers and asks one to accept this PC; a computer it accepted is then opened. */
    connector: Connector;
    /** Unlinks a computer, signing this PC out of it. */
    forget(id: string): Promise<void>;
};

type ProxyComputers = { computers: (LinkedComputer & { active: boolean })[]; local?: string | null };

async function proxyCall<T>(method: "GET" | "POST", path: string, body?: unknown): Promise<T> {
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

function shellSwitcher(invoke: NonNullable<ReturnType<typeof shellInvoke>>): ComputerSwitcher {
    return {
        list: () => invoke<LinkedComputer[]>("computers_list"),
        open: (id) => invoke("computers_open", { id }),
        home: async () => {},
        connector: {
            nearby: () => invoke<NearbyComputer[]>("computers_nearby"),
            ask: (address) => invoke<WaitingComputer>("computers_ask", { address }),
            answer: (id) => invoke<{ id: string }>("computers_answer", { id }),
            cancel: (id) => invoke("computers_cancel", { id }),
        },
        forget: (id) => invoke("computers_forget", { id }),
    };
}

function proxySwitcher(current: string | undefined): ComputerSwitcher {
    const showChosen = () => window.location.replace("/app");
    return {
        current,
        list: () => proxyCall<ProxyComputers>("GET", "/computers").then((value) => value.computers),
        open: async (id) => {
            await proxyCall("POST", "/use", { id });
            showChosen();
        },
        home: async () => {
            await proxyCall("POST", "/local");
        },
        connector: appConnector,
        forget: async (id) => {
            await proxyCall("POST", "/forget", { id });
            if (id === current) await proxyCall("POST", "/local");
        },
    };
}

/** The switcher for this window, or undefined where there is no PC to switch from: a browser, or the phone app. */
export async function computerSwitcher(): Promise<ComputerSwitcher | undefined> {
    const invoke = shellInvoke();
    if (invoke) return shellSwitcher(invoke);
    if (!isAppRuntime()) return undefined;

    const reported = await proxyCall<ProxyComputers>("GET", "/computers").catch(() => undefined);
    if (!reported?.local) return undefined;

    return proxySwitcher(reported.computers.find((computer) => computer.active)?.id);
}
