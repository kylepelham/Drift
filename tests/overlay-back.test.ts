import { closeMobileDrawer, openMobileDrawer, replaceRemoteSelection } from "../src/state/navigation";
import { afterEach, beforeEach, expect, test } from "bun:test";
import { bindOverlayBack } from "../src/state/overlay-back";

let restore: () => void;
let win: EventTarget;
let entries: Record<string, unknown>[];
let backs: number;

beforeEach(() => {
    const originalWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
    const originalHistory = Object.getOwnPropertyDescriptor(globalThis, "history");
    win = Object.assign(new EventTarget(), { location: { pathname: "/app", origin: "http://localhost" } });
    entries = [{ other: "preserved" }];
    backs = 0;
    const history = {
        get state() {
            return entries.at(-1);
        },
        pushState: (state: Record<string, unknown>) => entries.push(state),
        replaceState: (state: Record<string, unknown>, _title: string, url: string) => {
            entries[entries.length - 1] = state;
            Object.assign(win, { location: { pathname: "/app", origin: "http://localhost", hash: url } });
        },
        back: () => {
            backs++;
            entries.pop();
            win.dispatchEvent(new Event("popstate"));
        },
    };
    Object.defineProperty(globalThis, "window", { configurable: true, value: win });
    Object.defineProperty(globalThis, "history", { configurable: true, value: history });
    restore = () => {
        if (originalWindow) Object.defineProperty(globalThis, "window", originalWindow);
        else Reflect.deleteProperty(globalThis, "window");
        if (originalHistory) Object.defineProperty(globalThis, "history", originalHistory);
        else Reflect.deleteProperty(globalThis, "history");
    };
});

afterEach(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
    restore();
});

test("Android Back dismisses the overlay without leaving the page or discarding other history state", () => {
    let closed = 0;
    const cleanup = bindOverlayBack(() => closed++);
    expect(history.state.other).toBe("preserved");
    expect(entries.length).toBe(2);
    history.back();
    expect(closed).toBe(1);
    cleanup();
    expect(backs).toBe(1);
});

test("closing by a button removes its history entry without calling its close callback again", () => {
    let closed = 0;
    const cleanup = bindOverlayBack(() => closed++);
    cleanup();
    expect(closed).toBe(0);
    expect(backs).toBe(1);
    expect(entries).toEqual([{ other: "preserved" }]);
});

test("nested overlays close from the top without also closing their parent", () => {
    const closed: string[] = [];
    const parent = bindOverlayBack(() => closed.push("parent"));
    const child = bindOverlayBack(() => closed.push("child"));
    history.back();
    child();
    expect(closed).toEqual(["child"]);
    history.back();
    parent();
    expect(closed).toEqual(["child", "parent"]);
});

test("a new overlay waits for a closing overlay's asynchronous history traversal", async () => {
    const first = bindOverlayBack(() => {});
    const firstID = history.state.driftOverlay;
    const back = history.back;
    history.back = () => {
        backs++;
    };
    first();
    let closed = 0;
    const second = bindOverlayBack(() => closed++);
    expect(history.state.driftOverlay).toBe(firstID);
    entries.pop();
    win.dispatchEvent(new Event("popstate"));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(history.state.driftOverlay).not.toBe(firstID);
    expect(closed).toBe(0);
    history.back = back;
    second();
});

test("mirrored selection updates preserve the settings route and top dialog's Back marker", () => {
    Object.assign(win, { location: { pathname: "/app", origin: "http://localhost", hash: "#/overlay=settings" } });
    const cleanup = bindOverlayBack(() => {});
    const marker = history.state.driftOverlay;
    replaceRemoteSelection("workspace", "session");
    expect(history.state.driftOverlay).toBe(marker);
    expect(window.location.hash).toBe("#/workspace=workspace&session=session&overlay=settings");
    cleanup();
});

test("a page loaded on the drawer gets an entry under it, so closing the drawer stays on the page", () => {
    const at = (hash: string) =>
        Object.assign(win, { location: { pathname: "/app", origin: "http://localhost", hash } });
    at("#/overlay=drawer");
    history.pushState = (state: Record<string, unknown>, _title: string, url: string) => {
        entries.push(state);
        at(url);
    };
    openMobileDrawer();
    expect(entries.length).toBe(2);
    expect(window.location.hash).toContain("overlay=drawer");
    closeMobileDrawer();
    expect(backs).toBe(1);
    expect(entries.length).toBe(1);
});
