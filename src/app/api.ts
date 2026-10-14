/** The Drift app's own requests to its local proxy; the PCs themselves are reached only through it. */

export type Computer = { id: string; name: string; address?: string; words: string[]; active: boolean };
export type Found = {
    found: { name: string; address: string; port: number; fingerprint: string; api: number };
    linked: boolean;
};
export type Linking = { id: string; name: string; code: string; words: string[] };
export type LinkStatus = "waiting" | "expired" | "linked";

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

export const appApi = {
    computers: () => call<{ computers: Computer[] }>("GET", "/computers").then((value) => value.computers),
    discover: () => call<{ found: Found[] }>("POST", "/discover").then((value) => value.found),
    /** Opens the camera; resolves to the PC linked, or undefined when the user backed out. */
    scan: () =>
        call<{ linked?: string; cancelled?: boolean }>("POST", "/scan").then((value) => value.linked ?? undefined),
    link: (address: string, port?: number) => call<Linking>("POST", "/link", { address, port }),
    poll: (id: string) =>
        call<{ status: LinkStatus }>("GET", `/link/${encodeURIComponent(id)}`).then((value) => value.status),
    use: (id: string) => call<{ ok: true }>("POST", "/use", { id }),
    forget: (id: string) => call<{ ok: true }>("POST", "/forget", { id }),
};

/** Opens Drift on the PC just chosen or linked. */
export function openDrift() {
    window.location.replace("/app");
}
