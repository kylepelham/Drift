// Speaks the browser end of the DevTools Protocol to Drift over a loopback socket, through chrome.debugger. Edge
// started with a debugging port turns IE mode off, so this is how Drift drives the Edge tabs of an IE mode browser.
const socket = new WebSocket("__DRIFT_RELAY__");
// A message to Drift now and then keeps this service worker from being stopped as idle.
const KEEPALIVE_MS = 20000;
// How often tabs are looked at again for one that moved into or out of IE mode.
const SWEEP_MS = 1000;
// Edge refuses the debugger on a tab IE mode hosts, naming this address.
const IE_HOST = "internetexplorertabhost";

const sessions = new Map();
const attaching = new Set();
let attachAll = false;
let nextSession = 1;

function send(message) {
    if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify(message));
}

function sessionOf(tabId) {
    for (const [session, tab] of sessions) if (tab === tabId) return session;
    return undefined;
}

function targetInfo(tab) {
    return {
        targetId: String(tab.id),
        type: "page",
        url: tab.pendingUrl || tab.url || "",
        title: tab.title || "",
        attached: sessionOf(tab.id) !== undefined,
        openerId: tab.openerTabId === undefined ? "" : String(tab.openerTabId),
    };
}

function detached(tabId) {
    const sessionId = sessionOf(tabId);
    if (!sessionId) return;
    sessions.delete(sessionId);
    send({ method: "Target.detachedFromTarget", params: { sessionId, targetId: String(tabId) } });
}

async function attach(tab) {
    if (!attachAll || attaching.has(tab.id) || sessionOf(tab.id)) return;
    attaching.add(tab.id);
    try {
        await chrome.debugger.attach({ tabId: tab.id }, "1.3");
        const sessionId = `tab-${tab.id}-${nextSession++}`;
        sessions.set(sessionId, tab.id);
        const current = await chrome.tabs.get(tab.id);
        send({
            method: "Target.attachedToTarget",
            params: { sessionId, targetInfo: targetInfo(current), waitingForDebugger: false },
        });
    } catch (error) {
        if (String(error).includes(IE_HOST)) send({ method: "Drift.ieTab", params: targetInfo(tab) });
    } finally {
        attaching.delete(tab.id);
    }
}

const browser = {
    "Browser.getVersion": () => ({
        protocolVersion: "1.3",
        product: `Chrome/${(navigator.userAgent.match(/Chrome\/(\S+)/) ?? [])[1] ?? ""}`,
        userAgent: navigator.userAgent,
    }),
    "Browser.close": async () => {
        for (const window of await chrome.windows.getAll()) await chrome.windows.remove(window.id);
        return {};
    },
    "Target.setDiscoverTargets": () => ({}),
    "Target.setAutoAttach": async () => {
        attachAll = true;
        for (const tab of await chrome.tabs.query({})) attach(tab);
        return {};
    },
    "Target.getTargets": async () => {
        const tabs = await chrome.tabs.query({});
        return { targetInfos: tabs.filter((tab) => sessionOf(tab.id)).map(targetInfo) };
    },
    // A window of its own keeps a new tab from covering an IE mode tab, which shows nothing while covered.
    // It starts minimized, and Drift restores it off-screen, so it never shows on the user's screen.
    "Target.createTarget": async ({ url }) => {
        const window = await chrome.windows.create({ url, focused: false, state: "minimized" });
        return { targetId: String(window.tabs[0].id) };
    },
    "Target.closeTarget": async ({ targetId }) => {
        await chrome.tabs.remove(Number(targetId));
        return { success: true };
    },
    "Target.activateTarget": async ({ targetId }) => {
        await chrome.tabs.update(Number(targetId), { active: true });
        return {};
    },
    // Loads an address the site list sends to IE mode: a tab with the debugger on it would stay in Chromium.
    "Drift.load": async ({ targetId, url }) => {
        const tabId = Number(targetId);
        if (sessionOf(tabId)) {
            await chrome.debugger.detach({ tabId });
            detached(tabId);
        }
        await chrome.tabs.update(tabId, { url });
        return {};
    },
    // Brings an IE mode tab to the front of its window, where it is painted and so can be pictured.
    "Drift.showIeTab": async ({ url }) => {
        const tabs = await chrome.tabs.query({});
        const tab = tabs.find((candidate) => !sessionOf(candidate.id) && candidate.url === url);
        if (tab) await chrome.tabs.update(tab.id, { active: true });
        return { shown: Boolean(tab) };
    },
};

socket.onmessage = async (event) => {
    const { id, method, params = {}, sessionId } = JSON.parse(event.data);
    try {
        let result;
        if (sessionId) {
            result = await chrome.debugger.sendCommand({ tabId: sessions.get(sessionId) }, method, params);
        } else if (browser[method]) {
            result = await browser[method](params);
        } else {
            throw new Error(`${method} is not available in an IE mode browser`);
        }
        send({ id, result: result ?? {} });
    } catch (error) {
        send({ id, error: { message: String(error?.message ?? error) } });
    }
};

chrome.debugger.onEvent.addListener((source, method, params) => {
    // Frames and workers inside a page are never attached to, so only the page's own session reports.
    if (source.sessionId) return;
    const sessionId = sessionOf(source.tabId);
    if (sessionId) send({ method, params, sessionId });
});
chrome.debugger.onDetach.addListener((source) => detached(source.tabId));
chrome.tabs.onUpdated.addListener((tabId, change, tab) => {
    if (sessionOf(tabId)) {
        const moved = change.url !== undefined || change.title !== undefined;
        if (moved) send({ method: "Target.targetInfoChanged", params: { targetInfo: targetInfo(tab) } });
        return;
    }
    // A tab is attached once it has loaded, so the debugger never holds a page Edge is moving into IE mode.
    if (change.status === "complete") attach(tab);
});
chrome.tabs.onRemoved.addListener((tabId) => {
    send({ method: "Target.targetDestroyed", params: { targetId: String(tabId) } });
});
// Edge reports nothing for a tab that moves into IE mode, so tabs are looked at again now and then: one that left
// IE mode is attached, and one in it says so.
setInterval(async () => {
    for (const tab of await chrome.tabs.query({})) {
        if (tab.status !== "loading" && !sessionOf(tab.id)) attach(tab);
    }
}, SWEEP_MS);
setInterval(() => send({ method: "Drift.alive", params: {} }), KEEPALIVE_MS);
