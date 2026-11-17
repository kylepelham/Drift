import { nthSession, stepAround, VisitHistory } from "../src/state/session-nav";
import { stepMessageCursor } from "../src/state/message-cursor";
import { expect, test } from "bun:test";

const key = (key: string, modifiers: Partial<KeyboardEvent> = {}) =>
    ({ key, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...modifiers }) as KeyboardEvent;

test("every default shortcut is unique and written as eventCombo writes it", async () => {
    const { keybindDefs } = await import("../src/state/keybinds");
    const combos = keybindDefs.flatMap((def) => (def.combo ? [def.combo] : []));
    expect(new Set(combos).size).toBe(combos.length);
    for (const combo of combos) {
        const parts = combo.split("+");
        const modifiers = parts.slice(0, -1);
        expect(modifiers, combo).toEqual(["ctrl", "alt", "shift"].filter((part) => modifiers.includes(part)));
    }
});

test("the new defaults read from real key presses", async () => {
    const { eventCombo, sessionDigit } = await import("../src/state/keybinds");
    expect(eventCombo(key("F", { ctrlKey: true, shiftKey: true }))).toBe("ctrl+shift+f");
    expect(eventCombo(key("Tab", { ctrlKey: true, shiftKey: true }))).toBe("ctrl+shift+tab");
    expect(eventCombo(key("ArrowUp", { altKey: true, shiftKey: true }))).toBe("alt+shift+arrowup");
    expect(eventCombo(key(",", { ctrlKey: true }))).toBe("ctrl+,");
    expect(eventCombo(key("F2"))).toBe("f2");
    expect(sessionDigit(key("3", { ctrlKey: true }))).toBe(3);
    expect(sessionDigit(key("3", { ctrlKey: true, altKey: true })), "Ctrl+Alt+3 picks a computer").toBeUndefined();
    expect(sessionDigit(key("0", { ctrlKey: true })), "Ctrl+0 resets zoom").toBeUndefined();
});

test("Ctrl+digit counts sessions from the top, and 9 is always the last", () => {
    const sessions = ["a", "b", "c"];
    expect(nthSession(sessions, 1)).toBe("a");
    expect(nthSession(sessions, 3)).toBe("c");
    expect(nthSession(sessions, 4)).toBeUndefined();
    expect(nthSession(sessions, 9)).toBe("c");
});

test("next and previous session wrap, and start at an end when nothing is open", () => {
    const rows = ["a", "b", "c"];
    expect(stepAround(rows, 2, 1)).toBe("a");
    expect(stepAround(rows, 0, -1)).toBe("c");
    expect(stepAround(rows, -1, 1)).toBe("a");
    expect(stepAround(rows, -1, -1)).toBe("c");
    expect(stepAround([], -1, 1)).toBeUndefined();
});

test("back and forward walk visited sessions like a browser, skipping ones that are gone", () => {
    const history = new VisitHistory();
    for (const id of ["a", "b", "c"]) history.visit(id);
    const live = (id: string) => id !== "b";

    expect(history.step(-1, live)).toBe("a");
    expect(history.step(-1, live)).toBeUndefined();
    expect(history.step(1, live)).toBe("c");

    history.step(-1, () => true);
    history.visit("d");
    expect(
        history.step(1, () => true),
        "a new visit drops the forward entries",
    ).toBeUndefined();
    expect(history.step(-1, () => true)).toBe("b");
});

test("the message cursor starts on screen and then steps one message, or one prompt, at a time", () => {
    const order = ["u1", "a1", "u2", "a2", "u3", "a3"];
    const prompt = (id: string) => id.startsWith("u");

    expect(stepMessageCursor(order, null, -1, "a2")).toBe("a2");
    expect(stepMessageCursor(order, null, -1, undefined)).toBe("a3");
    expect(stepMessageCursor(order, "a2", -1, "a3")).toBe("u2");
    expect(stepMessageCursor(order, "a2", 1, undefined)).toBe("u3");
    expect(stepMessageCursor(order, "a3", 1, undefined)).toBeUndefined();

    expect(stepMessageCursor(order, null, -1, "a2", prompt)).toBe("u2");
    expect(stepMessageCursor(order, "u3", -1, undefined, prompt)).toBe("u2");
    expect(stepMessageCursor(order, "gone", 1, "a1", prompt)).toBe("u2");
});
