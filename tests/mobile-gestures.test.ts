import { afterEach, beforeEach, expect, test } from "bun:test";
import { drawerSwipe } from "../src/ui/drawer-swipe";

class Surface {
    parentElement: Surface | null = null;
    scrollWidth = 100;
    clientWidth = 100;
    overflowX = "visible";
    tag = "";
    closest(selector: string) {
        return this.tag && selector.split(",").some((part) => part.trim() === this.tag) ? this : null;
    }
}

let doc: EventTarget;
let cleanup: () => void;
let restore: () => void;
let open: boolean;
let props: Map<string, string>;

beforeEach(() => {
    const names = ["document", "Element", "getComputedStyle"] as const;
    const saved = names.map((name) => Object.getOwnPropertyDescriptor(globalThis, name));
    doc = new EventTarget();
    const values = [doc, Surface, (el: Surface) => ({ overflowX: el.overflowX })];
    names.forEach((name, index) =>
        Object.defineProperty(globalThis, name, { configurable: true, value: values[index] }),
    );
    restore = () =>
        names.forEach((name, index) => {
            if (saved[index]) Object.defineProperty(globalThis, name, saved[index]!);
            else Reflect.deleteProperty(globalThis, name);
        });
    open = false;
    props = new Map();
    const style = {
        set transform(value: string) {
            props.set("transform", value);
        },
        set transition(value: string) {
            props.set("transition", value);
        },
        set visibility(value: string) {
            props.set("visibility", value);
        },
        removeProperty: (name: string) => props.delete(name),
    };
    cleanup = drawerSwipe({ style, offsetWidth: 300 } as unknown as HTMLElement, {
        open: () => open,
        show: () => {
            open = true;
        },
        hide: () => {
            open = false;
        },
        narrow: () => true,
    });
});

afterEach(() => {
    cleanup();
    restore();
});

function touch(type: string, x: number, y: number, target: Surface, count = 1) {
    const event = new Event(type, { cancelable: true });
    Object.defineProperties(event, {
        touches: { value: Array.from({ length: count }, () => ({ clientX: x, clientY: y })) },
        changedTouches: { value: [{ clientX: x, clientY: y }] },
        target: { value: target },
    });
    doc.dispatchEvent(event);
    return event;
}

test("a horizontal page swipe opens the drawer but the Android Back edge stays untouched", () => {
    const target = new Surface();
    touch("touchstart", 50, 100, target);
    expect(touch("touchmove", 200, 100, target).defaultPrevented).toBeTrue();
    touch("touchend", 200, 100, target);
    expect(open).toBeTrue();
    open = false;
    touch("touchstart", 5, 100, target);
    expect(touch("touchmove", 200, 100, target).defaultPrevented).toBeFalse();
    touch("touchend", 200, 100, target);
    expect(open).toBeFalse();
});

test("controls and horizontally scrollable content keep their own gestures, even at scrollLeft zero", () => {
    const control = new Surface();
    control.tag = "input";
    const code = new Surface();
    code.scrollWidth = 1000;
    code.overflowX = "auto";
    for (const target of [control, code]) {
        touch("touchstart", 50, 100, target);
        expect(touch("touchmove", 250, 100, target).defaultPrevented).toBeFalse();
        touch("touchend", 250, 100, target);
        expect(open).toBeFalse();
    }
});

test("vertical scrolling and a second finger cancel the drawer without leaving inline drag styles", () => {
    const target = new Surface();
    touch("touchstart", 50, 100, target);
    expect(touch("touchmove", 55, 200, target).defaultPrevented).toBeFalse();
    touch("touchend", 200, 200, target);
    expect(open).toBeFalse();
    touch("touchstart", 50, 100, target);
    touch("touchmove", 200, 100, target);
    expect(props.has("transform")).toBeTrue();
    touch("touchmove", 250, 100, target, 2);
    touch("touchend", 250, 100, target);
    expect(open).toBeFalse();
    expect(props.size).toBe(0);
});

test("a swipe that starts on a thread row closes the drawer and the row is not opened", () => {
    const row = new Surface();
    row.tag = "button";
    open = true;
    touch("touchstart", 250, 300, row);
    expect(touch("touchmove", 150, 340, row).defaultPrevented).toBeTrue();
    touch("touchend", 100, 350, row);
    expect(open).toBeFalse();

    const tap = new Event("click", { cancelable: true });
    doc.dispatchEvent(tap);
    expect(tap.defaultPrevented).toBeTrue();
});
