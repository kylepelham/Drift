import { modifierBits, pagePoint, isPasteShortcut, tabLabel } from "../src/ui/browser-input";
import { decodeFrame } from "../src/engine/browser-view";
import { expect, test } from "bun:test";

function frameMessage(header: object, jpeg: number[]) {
    const json = new TextEncoder().encode(JSON.stringify(header));
    const message = new Uint8Array(4 + json.length + jpeg.length);
    new DataView(message.buffer).setUint32(0, json.length, true);
    message.set(json, 4);
    message.set(jpeg, 4 + json.length);

    return message.buffer;
}

test("a frame message is read as its header and its picture", async () => {
    const frame = decodeFrame(frameMessage({ page: "T1", width: 1280, height: 800 }, [0xff, 0xd8, 0xff]));

    expect(frame?.page).toBe("T1");
    expect(frame?.width).toBe(1280);
    expect([...new Uint8Array(await frame!.jpeg.arrayBuffer())]).toEqual([0xff, 0xd8, 0xff]);
    expect(decodeFrame(new ArrayBuffer(2))).toBeUndefined();
    expect(decodeFrame(frameMessage({ page: "T1" }, []).slice(0, 6))).toBeUndefined();
});

test("a click on the shrunken picture lands on the page's own pixels", () => {
    const shown = { left: 100, top: 50, width: 640, height: 400 };
    const page = { width: 1280, height: 800 };

    expect(pagePoint({ clientX: 420, clientY: 250 }, shown, page)).toEqual({ x: 640, y: 400 });
    expect(pagePoint({ clientX: 100, clientY: 50 }, shown, page)).toEqual({ x: 0, y: 0 });
});

test("modifiers, paste and tab names", () => {
    expect(modifierBits({ altKey: true, ctrlKey: true, metaKey: false, shiftKey: true })).toBe(11);
    expect(isPasteShortcut({ key: "V", ctrlKey: true, metaKey: false })).toBe(true);
    expect(isPasteShortcut({ key: "v", ctrlKey: false, metaKey: false })).toBe(false);
    expect(tabLabel({ title: "", url: "http://localhost:5173/menu" })).toBe("localhost:5173");
    expect(tabLabel({ title: "Menu", url: "http://x/" })).toBe("Menu");
});
