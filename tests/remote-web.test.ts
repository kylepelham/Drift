import { appRuntimeFrom, remoteEngineBase, remoteRuntimeFrom, runtimeNameFrom } from "../src/runtime";
import { isNarrowWidth, navigationHash, parseNavigationHash } from "../src/state/navigation";
import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { backendInvoke, backendRoute } from "../src/backend";
import {
    nextRemoteAccessEnabled,
    pairedDevice,
    pairingReachesAnywhere,
    remoteStatusTone,
    tunnelUrl,
    type RemoteAccessStatus,
} from "../src/state/remote-access";

test("remote runtime uses the same-origin engine gateway", () => {
    const remote = { pathname: "/companion", origin: "https://192.168.1.8:41718" };
    const desktop = { pathname: "/", origin: "http://localhost:5180" };
    expect(remoteRuntimeFrom(remote)).toBe(true);
    expect(remoteRuntimeFrom(desktop)).toBe(false);
    expect(remoteEngineBase(remote)).toBe("https://192.168.1.8:41718/engine");
    expect(remoteEngineBase(desktop)).toBeUndefined();
});

test("the Drift app's proxy serves the UI as a remote one under /app, with the engine beside it", () => {
    const app = { pathname: "/app", origin: "http://127.0.0.1:32999" };

    expect(appRuntimeFrom(app)).toBeTrue();
    expect(appRuntimeFrom({ ...app, pathname: "/app/computers" })).toBeTrue();
    expect(appRuntimeFrom({ ...app, pathname: "/application" })).toBeFalse();
    expect(remoteRuntimeFrom(app)).toBeTrue();
    expect(runtimeNameFrom(app)).toBe("remote");
    expect(remoteEngineBase(app)).toBe("http://127.0.0.1:32999/engine");
    expect(appRuntimeFrom({ ...app, pathname: "/companion" })).toBeFalse();
});

test("dynamic viewport sizing is limited to the remote runtime", () => {
    expect(runtimeNameFrom({ pathname: "/companion", origin: "https://192.168.1.20:41718" })).toBe("remote");
    expect(runtimeNameFrom({ pathname: "/", origin: "tauri://localhost" })).toBe("desktop");
});

test("host backend routing prefers Tauri and otherwise uses remote RPC", () => {
    expect(backendRoute(true, true)).toBe("tauri");
    expect(backendRoute(false, true)).toBe("rpc");
    expect(backendRoute(false, false)).toBe("browser");
});

let fetchMock: ReturnType<typeof mock>;
let replaceMock: ReturnType<typeof mock>;
const globalNames = ["window", "location", "fetch"] as const;
let originalGlobals: (PropertyDescriptor | undefined)[];

function setGlobal(name: (typeof globalNames)[number], value: unknown) {
    Object.defineProperty(globalThis, name, { configurable: true, writable: true, value });
}

beforeEach(() => {
    originalGlobals = globalNames.map((name) => Object.getOwnPropertyDescriptor(globalThis, name));
    fetchMock = mock(async (url: string) =>
        Response.json(url === "/auth/me" ? { id: "device" } : { error: "remote access credentials changed" }, {
            status: url === "/auth/me" ? 200 : 401,
        }),
    );
    replaceMock = mock(() => {});
    const location = {
        ...new URL("https://192.168.1.8:41718/companion"),
        pathname: "/companion",
        origin: "https://192.168.1.8:41718",
        replace: replaceMock,
    };
    setGlobal("fetch", fetchMock);
    setGlobal("location", location);
    setGlobal("window", { location });
});

afterEach(() => {
    for (const [index, name] of globalNames.entries()) {
        const descriptor = originalGlobals[index];
        if (descriptor) Object.defineProperty(globalThis, name, descriptor);
        else delete (globalThis as Record<string, unknown>)[name];
    }
});

test("remote RPC 401 confirms the current session before redirecting", async () => {
    const invoke = backendInvoke()!;
    await expect(invoke("read_file_preview", { path: "C:/tmp/a.png" })).rejects.toThrow(
        "remote access credentials changed",
    );
    expect(fetchMock.mock.calls.map(([url]) => url)).toEqual(["/api/invoke", "/auth/me"]);
    expect(replaceMock).not.toHaveBeenCalled();
});

test("remote RPC redirects only when the current session is invalid", async () => {
    fetchMock.mockImplementation(async (url: string) =>
        Response.json(url === "/auth/me" ? {} : { error: "remote access credentials changed" }, { status: 401 }),
    );
    const invoke = backendInvoke()!;
    await expect(invoke("read_file_preview", { path: "C:/tmp/a.png" })).rejects.toThrow(
        "remote access credentials changed",
    );
    expect(fetchMock.mock.calls.map(([url]) => url)).toEqual(["/api/invoke", "/auth/me"]);
    expect(replaceMock.mock.calls).toEqual([["/companion"]]);
});

test("responsive navigation state round-trips and uses the narrow breakpoint", () => {
    expect(isNarrowWidth(390)).toBe(true);
    expect(isNarrowWidth(720)).toBe(false);
    const hash = navigationHash({ workspace: "project A", session: "ses_1", overlay: "drawer" });
    expect(parseNavigationHash(hash)).toEqual({ workspace: "project A", session: "ses_1", overlay: "drawer" });
});

test("mobile layout keeps scrolling inside the transcript and drawer", async () => {
    const app = await Bun.file("src/app.tsx").text();
    const css = await Bun.file("src/styles/app.css").text();
    const mobile = await Bun.file("src/styles/mobile.css").text();
    const parts = (
        await Promise.all(
            ["tool-body", "shell-output", "diff-panel"].map((name) => Bun.file(`src/ui/${name}.tsx`).text()),
        )
    ).join("\n");
    const markdown = await Bun.file("src/ui/progressive-code.tsx").text();
    const composer = await Bun.file("src/ui/composer.tsx").text();
    const sidebar = await Bun.file("src/ui/sidebar.tsx").text();
    const workspaces = await Bun.file("src/ui/workspaces.tsx").text();

    expect(css).toMatch(/html,\s*body,\s*#root\s*\{[^}]*overflow: hidden/s);
    expect(css).toMatch(/\.app-shell\s*\{[^}]*overflow: hidden/s);
    expect(css).toMatch(/\.transcript-scroll\s*\{[^}]*overscroll-behavior-y: contain/s);
    expect(mobile).toMatch(/@media \(max-width: 719px\)[\s\S]*\.chat-pane-covered\s*\{[^}]*visibility: hidden/s);
    expect(await Bun.file("src/ui/debug.tsx").text()).toContainCode("debug-panel-scroll");
    expect(await Bun.file("src/app.tsx").text()).toContainCode(
        "(rightPanelOpen() || browserPaneShowing()) && !!selectedSession()",
    );
    expect(css).toMatch(/\.composer-dock\s*\{[^}]*padding-top: 1rem/s);
    expect(css).toMatch(/\.composer-dock::before\s*\{[^}]*inset: -2\.75rem 0 auto;[^}]*linear-gradient/s);
    expect(css).toMatch(/\.dock-card\s*\{[^}]*box-shadow:/s);
    expect(app.match(/min-h-0 min-w-0 flex-1[^\"]*overflow-hidden/g)).toHaveLength(2);
    expect(parts.match(/transcript-tool-output/g)?.length).toBeGreaterThanOrEqual(5);
    expect(markdown).toContainCode("transcript-tool-output code-view code-stream");
    expect(mobile).toMatch(
        /@media \(max-width: 719px\)[\s\S]*\.transcript-scroll\.transcript-scroll-active \.transcript-tool-output\s*\{[^}]*pointer-events: none/s,
    );
    expect(css).not.toMatch(/\.transcript-tool-output\s*\{[^}]*max-height: none/s);
    expect(await Bun.file("src/ui/chat.tsx").text()).toContainCode('classList.add("transcript-scroll-active")');
    expect(composer).toContainCode('class="composer-options relative flex min-w-0 flex-1');
    expect(composer).not.toContainCode("composer-options flex min-w-0 flex-1 items-center gap-1 overflow-hidden");
    expect(composer).toContainCode('class="composer-action-buttons ml-auto flex shrink-0');
    expect(mobile).not.toMatch(/\.composer-actions\s*\{[^}]*flex-wrap: wrap/s);
    expect(mobile).not.toMatch(/\.composer-options\s*\{[^}]*flex: 1 0 100%/s);
    expect(mobile).toMatch(
        /\.composer-options \.picker-chevron,\s*\.composer-options \.picker-label-full\s*\{\s*display: none/s,
    );
    expect(mobile).toMatch(/\.composer-options > \.picker-control\s*\{[^}]*min-width: 0/s);
    expect(css).not.toMatch(
        /\.composer-actions button,\s*\.composer-actions \[role="button"\]\s*\{[^}]*min-height: 0/s,
    );
    expect(mobile).toMatch(
        /\.composer-shell \.composer-actions button\s*\{[^}]*min-width: 2\.25rem;[^}]*min-height: 2\.25rem/s,
    );
    // The drawer must not add safe-area padding: the wrapper webview already sits below the
    // status bar, so env(safe-area-inset-top) doubles into a large blank band at the top.
    expect(mobile).not.toMatch(/\.app-sidebar\s*\{[^}]*safe-area-inset-top/s);
    expect(mobile).toMatch(/\.app-sidebar\s*\{[^}]*visibility: hidden;[^}]*visibility 0s linear 180ms/s);
    expect(mobile).toMatch(/\.app-sidebar\.mobile-sidebar-open\s*\{[^}]*visibility: visible/s);
    expect(mobile).toMatch(/\.app-sidebar-scroll\s*\{[^}]*touch-action: pan-y/s);
    expect(sidebar).toContainCode('event.target.closest("[data-sidebar-navigation]")');
    expect(workspaces).toContainCode('class="group sticky top-0');
});

test("remote settings state distinguishes online, offline, and error", () => {
    const base = remoteStatus();
    expect(remoteStatusTone(base)).toBe("online");
    expect(remoteStatusTone({ ...base, listening: false })).toBe("offline");
    expect(remoteStatusTone({ ...base, error: "busy" })).toBe("error");
    expect(nextRemoteAccessEnabled(base)).toBeFalse();
    expect(nextRemoteAccessEnabled({ ...base, enabled: false })).toBeTrue();
});

test("remote access toggle has one state transition helper", async () => {
    const section = await Bun.file("src/ui/settings-remote-access.tsx").text();
    expect(section).toContainCode(
        "const toggle = () => !remoteAccessBusy() && void setRemoteAccess(nextRemoteAccessEnabled(status()));",
    );
    expect(section.match(/setRemoteAccess\(/g)).toHaveLength(1);
    expect(section).not.toContainCode("remote-access-card");
});

test("the pairing code counts as used once an app device signs in after it was shown", () => {
    const device = (method: "link" | "password" | "app", createdAt: number) => ({
        id: `${method}${createdAt}`,
        name: `${method} device`,
        method,
        createdAt,
        lastSeenAt: createdAt,
    });
    const status = (devices: ReturnType<typeof device>[]) => remoteStatus({ devices });

    expect(pairedDevice(status([device("app", 5), device("link", 20)]), 10)).toBeUndefined();
    expect(pairedDevice(status([device("app", 5), device("app", 12)]), 10)?.id).toBe("app12");
    expect(pairedDevice(null, 10)).toBeUndefined();
});

test("the tunnel address shows only while it carries traffic, and a code says whether it reaches past the network", () => {
    const url = "https://k2v7qxmd4hzt6wnc.x3fp8rk2mq7a.opentunnel.xyz";
    const tunnel = (phase: RemoteAccessStatus["tunnel"]["phase"]) => remoteStatus({ tunnel: { phase, url } });

    expect(tunnelUrl(tunnel("online"))).toBe(url);
    expect(tunnelUrl(tunnel("reconnecting"))).toBe(url);
    expect(tunnelUrl(tunnel("setting-up"))).toBeUndefined();
    expect(tunnelUrl(tunnel("failed"))).toBeUndefined();
    expect(tunnelUrl(null)).toBeUndefined();

    const code = (query: string) => ({ uri: `drift://pair?v=1&a=192.168.1.20${query}&t=ab`, qr: "", expiresAt: 0 });
    expect(pairingReachesAnywhere(code("&h=k2v7.x3fp8rk2mq7a.opentunnel.xyz&k=5a"))).toBeTrue();
    expect(pairingReachesAnywhere(code(""))).toBeFalse();
});

test("the shared access-key flow is gone and management stays desktop-only", async () => {
    const state = await Bun.file("src/state/remote-access.ts").text();
    const section = await Bun.file("src/ui/settings-remote-access.tsx").text();
    for (const source of [state, section]) {
        expect(source).not.toContainCode("connectionUrls");
        expect(source).not.toContainCode("rotate");
        expect(source).not.toContainCode("?token=");
    }
    expect(section).toContainCode("<Show when={!isRemoteRuntime()} fallback={<ThisDevice />}>");
    expect(await Bun.file("src/backend.ts").text()).toContainCode(
        "if (response.status === 401 && (await remoteSessionExpired())) window.location.replace(signInPath())",
    );
});

test("device activity reads as relative time", async () => {
    const { lastSeenLabel } = await import("../src/state/remote-access");
    const now = Date.UTC(2026, 8, 28, 12);
    const english = (at: number) => lastSeenLabel(at, now);
    expect(english(now - 20_000)).toBe(new Intl.RelativeTimeFormat(undefined, { numeric: "auto" }).format(0, "minute"));
    expect(english(now - 5 * 60_000)).toBe(
        new Intl.RelativeTimeFormat(undefined, { numeric: "auto" }).format(-5, "minute"),
    );
    expect(english(now - 3 * 86_400_000)).toBe(
        new Intl.RelativeTimeFormat(undefined, { numeric: "auto" }).format(-3, "day"),
    );
});

test("devices link only by the app's pairing code; browsers sign in with a password behind their own switch", async () => {
    const section = await Bun.file("src/ui/settings-remote-access.tsx").text();
    const state = await Bun.file("src/state/remote-access.ts").text();
    for (const gone of ["remote-link-code", "remote_access_link", "pendingLinks", "addressQr", "certificateWords"]) {
        expect(section + state).not.toContain(gone);
    }
    expect(state).toContainCode('call("remote_access_set_browser", { enabled })');
    expect(state).toContainCode('call("remote_access_set_tunnel", { enabled })');
    expect(section).toContainCode("<Show when={status()?.browser}>");
    expect(await Bun.file("src/app.tsx").text()).not.toContain("RemoteLinkNotice");
});

test("in the companion the native client reaches the engine through the gateway and holds no engine token", async () => {
    const saved = (globalThis as { window?: unknown }).window;
    Object.defineProperty(globalThis, "window", {
        configurable: true,
        value: { location: { pathname: "/companion", origin: "https://192.168.1.8:41718" } },
    });
    try {
        const { resolveTarget } = await import("../src/engine/native/target");
        expect(await resolveTarget()).toEqual({ url: "https://192.168.1.8:41718/engine", token: "" });
    } finally {
        Object.defineProperty(globalThis, "window", { configurable: true, value: saved });
    }
});

function remoteStatus(patch: Partial<RemoteAccessStatus> = {}): RemoteAccessStatus {
    return {
        enabled: true,
        listening: true,
        browser: false,
        tunnelEnabled: false,
        tunnel: { phase: "off" },
        devices: [],
        certificateFingerprint: "AB:CD",
        ...patch,
    };
}
