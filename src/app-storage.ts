/** A request the page may not outlive must stay under the browser's 64 KB keepalive budget. */
const KEEPALIVE_BYTES = 60_000;

type Values = Record<string, string>;
/** Sends a snapshot, or with `base` only the changes since that acknowledged one; resolves whether the app kept it. */
type Send = (body: string, sequence: number, base?: number) => Promise<boolean> | void;

/** Sends the phone app every change to saved values, which it restores next launch (`docs/android.md`). */
export function keepAppStorage(storage: Storage = localStorage, send: Send = sendToApp, proto = Storage.prototype) {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let sent = 0;
    let acked: { sequence: number; values: Values } | undefined;
    const flush = () => {
        clearTimeout(timer);
        timer = undefined;
        const values: Values = Object.fromEntries(entries(storage));
        const sequence = ++sent;
        const whole = JSON.stringify(values);
        // A large set goes as its changes since the last kept snapshot, so it can still outlive the page.
        const base = bytes(whole) >= KEEPALIVE_BYTES ? acked : undefined;
        const body = base ? JSON.stringify(changes(base.values, values)) : whole;
        void Promise.resolve(send(body, sequence, base?.sequence)).then((kept) => {
            if (kept && (!acked || acked.sequence < sequence)) acked = { sequence, values };
        });
    };
    const later = () => {
        clearTimeout(timer);
        timer = setTimeout(flush, 300);
    };

    for (const name of ["setItem", "removeItem", "clear"] as const) {
        const original = proto[name] as (this: Storage, ...args: unknown[]) => void;
        proto[name] = function (this: Storage, ...args: unknown[]) {
            original.apply(this, args);
            if (this === storage) later();
        } as never;
    }
    document.addEventListener("visibilitychange", () => {
        if (document.visibilityState === "hidden" && timer) flush();
    });
    // Values written while the page loaded, before this ran, are kept too.
    later();
}

function entries(storage: Storage) {
    const found: [string, string][] = [];
    for (let index = 0; index < storage.length; index++) {
        const key = storage.key(index);
        const value = key === null ? null : storage.getItem(key);
        if (key !== null && value !== null) found.push([key, value]);
    }

    return found;
}

function changes(before: Values, after: Values) {
    const set = Object.fromEntries(Object.entries(after).filter(([key, value]) => before[key] !== value));
    const removed = Object.keys(before).filter((key) => !(key in after));

    return { set, removed };
}

function bytes(text: string) {
    return new TextEncoder().encode(text).length;
}

/** This page's name to the app, so a slow earlier snapshot of it never replaces a later one. */
const page = Math.random().toString(36).slice(2);

function sendToApp(body: string, sequence: number, base?: number) {
    const since = base === undefined ? "" : `&base=${base}`;

    return fetch(`/app/api/storage?page=${page}&sequence=${sequence}${since}`, {
        method: "PUT",
        credentials: "same-origin",
        headers: { "content-type": "application/json" },
        body,
        keepalive: bytes(body) < KEEPALIVE_BYTES,
    })
        .then((response) => response.ok)
        .catch(() => false);
}
