/** Sends the phone app every change to saved values, which it restores next launch (`docs/android.md`). */
export function keepAppStorage(storage: Storage = localStorage, send = sendToApp, proto = Storage.prototype) {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let sent = 0;
    const flush = () => {
        clearTimeout(timer);
        timer = undefined;
        send(JSON.stringify(Object.fromEntries(entries(storage))), ++sent);
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

/** This page's name to the app, so a slow earlier snapshot of it never replaces a later one. */
const page = Math.random().toString(36).slice(2);

function sendToApp(body: string, sequence: number) {
    // A request the page may not outlive must stay under the browser's 64 KB keepalive budget.
    void fetch(`/app/api/storage?page=${page}&sequence=${sequence}`, {
        method: "PUT",
        credentials: "same-origin",
        headers: { "content-type": "application/json" },
        body,
        keepalive: new TextEncoder().encode(body).length < 60_000,
    }).catch(() => undefined);
}
